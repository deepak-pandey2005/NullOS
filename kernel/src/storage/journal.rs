//! # Transactional Write-Ahead Log (WAL) Journaling Subsystem (Phase 15 - M35)
//!
//! ## Overview
//! Provides crash-consistent metadata and data journaling via Write-Ahead Logging (WAL).
//! Before modifying persistent filesystem blocks in-place, callers group sector mutations
//! inside a [`Journal`] transaction:
//! 1. `begin_tx()` allocates a monotonic Transaction ID (`tx_id`).
//! 2. `log_write(tx_id, sector, data)` records each intended sector mutation along with an
//!    IEEE 802.3 CRC32 checksum inside the journal log.
//! 3. `commit_tx(tx_id)` appends a Commit record, making the transaction durable.
//! 4. `checkpoint(device)` flushes committed transactions out to their final disk sectors,
//!    flushes hardware caches, and marks the transactions checkpointed.
//! 5. `recover(device)` replays all CRC-verified `Committed` (uncheckpointed) transactions
//!    after an unexpected crash while safely discarding incomplete/uncommitted transactions.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::block::{BlockDevice, BlockError, SECTOR_SIZE};
use crate::sync::Spinlock;

/// Magic signature identifying valid NullOS Journal records ("NJRN").
pub const JOURNAL_MAGIC: u32 = 0x4E4A_524E;

/// Lifecycle state of a journal transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Active,
    Committed,
    Aborted,
    Checkpointed,
}

/// On-disk / serialized WAL record header for a single journaled block write.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JournalBlockRecordHeader {
    pub magic: u32,
    pub tx_id: u64,
    pub seq: u64,
    pub target_sector: u64,
    pub data_len: u32,
    pub crc32: u32,
}

/// Single logged sector write inside a transaction.
#[derive(Debug, Clone)]
pub struct JournalBlockEntry {
    pub header: JournalBlockRecordHeader,
    pub data: Vec<u8>,
}

/// Complete journal transaction record.
#[derive(Debug, Clone)]
pub struct JournalTransaction {
    pub tx_id: u64,
    pub state: TransactionState,
    pub entries: Vec<JournalBlockEntry>,
}

/// Summary returned after running crash recovery replay.
#[derive(Debug, Clone, Copy, Default)]
pub struct JournalRecoveryReport {
    pub committed_replayed: usize,
    pub blocks_replayed: usize,
    pub uncommitted_discarded: usize,
    pub corrupted_rejected: usize,
}

/// Telemetry counters for the Write-Ahead Log subsystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct JournalStats {
    pub next_tx_id: u64,
    pub active_txs: usize,
    pub committed_pending_checkpoint: usize,
    pub total_committed: u64,
    pub total_aborted: u64,
    pub total_checkpointed: u64,
    pub total_logged_blocks: u64,
    pub total_replayed_blocks: u64,
}

struct JournalInner {
    next_tx_id: u64,
    next_seq: u64,
    transactions: BTreeMap<u64, JournalTransaction>,
    total_committed: u64,
    total_aborted: u64,
    total_checkpointed: u64,
    total_logged_blocks: u64,
    total_replayed_blocks: u64,
}

/// Thread-safe Transactional Write-Ahead Journal.
pub struct Journal {
    inner: Spinlock<JournalInner>,
}

/// Computes standard IEEE 802.3 CRC32 checksum over `data` for journal integrity checks.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & mask);
        }
    }
    !crc
}

impl Journal {
    /// Creates an empty Write-Ahead Log Journal instance.
    pub const fn new() -> Self {
        Self {
            inner: Spinlock::new(JournalInner {
                next_tx_id: 1,
                next_seq: 1,
                transactions: BTreeMap::new(),
                total_committed: 0,
                total_aborted: 0,
                total_checkpointed: 0,
                total_logged_blocks: 0,
                total_replayed_blocks: 0,
            }),
        }
    }

    /// Starts a new atomic transaction and returns its unique `tx_id`.
    pub fn begin_tx(&self) -> u64 {
        let mut guard = self.inner.lock();
        let tx_id = guard.next_tx_id;
        guard.next_tx_id += 1;
        guard.transactions.insert(
            tx_id,
            JournalTransaction {
                tx_id,
                state: TransactionState::Active,
                entries: Vec::new(),
            },
        );
        tx_id
    }

    /// Appends a sector write intent to an active transaction, computing its CRC32 checksum.
    pub fn log_write(&self, tx_id: u64, target_sector: u64, data: &[u8]) -> Result<(), BlockError> {
        let mut guard = self.inner.lock();
        let seq = guard.next_seq;
        guard.next_seq += 1;

        let tx = guard.transactions.get_mut(&tx_id).ok_or(BlockError::InvalidSector)?;
        if tx.state != TransactionState::Active {
            return Err(BlockError::Unsupported);
        }

        let mut buf = vec![0u8; core::cmp::max(data.len(), SECTOR_SIZE)];
        buf[..data.len()].copy_from_slice(data);
        let checksum = crc32(&buf);

        tx.entries.push(JournalBlockEntry {
            header: JournalBlockRecordHeader {
                magic: JOURNAL_MAGIC,
                tx_id,
                seq,
                target_sector,
                data_len: data.len() as u32,
                crc32: checksum,
            },
            data: buf,
        });
        guard.total_logged_blocks += 1;
        Ok(())
    }

    /// Marks an active transaction as Committed, making all its logged writes eligible for checkpoint/replay.
    pub fn commit_tx(&self, tx_id: u64) -> Result<(), BlockError> {
        let mut guard = self.inner.lock();
        let tx = guard.transactions.get_mut(&tx_id).ok_or(BlockError::InvalidSector)?;
        if tx.state != TransactionState::Active {
            return Err(BlockError::Unsupported);
        }
        tx.state = TransactionState::Committed;
        guard.total_committed += 1;
        Ok(())
    }

    /// Aborts an active transaction and discards all its uncommitted writes.
    pub fn abort_tx(&self, tx_id: u64) -> Result<(), BlockError> {
        let mut guard = self.inner.lock();
        let tx = guard.transactions.get_mut(&tx_id).ok_or(BlockError::InvalidSector)?;
        if tx.state != TransactionState::Active {
            return Err(BlockError::Unsupported);
        }
        tx.state = TransactionState::Aborted;
        tx.entries.clear();
        guard.total_aborted += 1;
        Ok(())
    }

    /// Checkpoints all `Committed` transactions to their final home sectors on `device`,
    /// flushes the device cache, and transitions them to `Checkpointed`.
    pub fn checkpoint(&self, device: &dyn BlockDevice) -> Result<usize, BlockError> {
        let mut guard = self.inner.lock();
        let committed_ids: Vec<u64> = guard
            .transactions
            .iter()
            .filter_map(|(&id, tx)| if tx.state == TransactionState::Committed { Some(id) } else { None })
            .collect();

        let mut applied_blocks = 0usize;
        for id in committed_ids {
            if let Some(tx) = guard.transactions.get_mut(&id) {
                for entry in tx.entries.iter() {
                    if entry.header.magic != JOURNAL_MAGIC || crc32(&entry.data) != entry.header.crc32 {
                        return Err(BlockError::IoError);
                    }
                    device.write_sector(entry.header.target_sector, &entry.data)?;
                    applied_blocks += 1;
                }
                tx.state = TransactionState::Checkpointed;
                tx.entries.clear();
                guard.total_checkpointed += 1;
            }
        }
        device.flush()?;
        Ok(applied_blocks)
    }

    /// Performs crash recovery on the journal:
    /// - Replays any `Committed` transactions that were not yet checkpointed (verifying CRC32 & magic).
    /// - Discards any `Active` (incomplete / uncommitted) transactions.
    pub fn recover(&self, device: &dyn BlockDevice) -> Result<JournalRecoveryReport, BlockError> {
        let mut guard = self.inner.lock();
        let mut report = JournalRecoveryReport::default();

        let mut replayed_blocks_delta = 0u64;
        let mut checkpointed_delta = 0u64;
        let mut aborted_delta = 0u64;

        let ids: Vec<u64> = guard.transactions.keys().copied().collect();
        for id in ids {
            if let Some(tx) = guard.transactions.get_mut(&id) {
                match tx.state {
                    TransactionState::Committed => {
                        let mut valid = true;
                        for entry in tx.entries.iter() {
                            if entry.header.magic != JOURNAL_MAGIC || crc32(&entry.data) != entry.header.crc32 {
                                valid = false;
                                report.corrupted_rejected += 1;
                                break;
                            }
                        }
                        if valid {
                            for entry in tx.entries.iter() {
                                device.write_sector(entry.header.target_sector, &entry.data)?;
                                report.blocks_replayed += 1;
                                replayed_blocks_delta += 1;
                            }
                            report.committed_replayed += 1;
                            tx.state = TransactionState::Checkpointed;
                            tx.entries.clear();
                            checkpointed_delta += 1;
                        } else {
                            tx.state = TransactionState::Aborted;
                            tx.entries.clear();
                            aborted_delta += 1;
                        }
                    }
                    TransactionState::Active => {
                        tx.state = TransactionState::Aborted;
                        tx.entries.clear();
                        report.uncommitted_discarded += 1;
                        aborted_delta += 1;
                    }
                    TransactionState::Aborted | TransactionState::Checkpointed => {}
                }
            }
        }

        guard.total_replayed_blocks += replayed_blocks_delta;
        guard.total_checkpointed += checkpointed_delta;
        guard.total_aborted += aborted_delta;

        device.flush()?;
        Ok(report)
    }

    /// Injects a corrupted checksum into an active transaction's last entry (for crash-test validation).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn corrupt_last_entry_for_test(&self, tx_id: u64) {
        let mut guard = self.inner.lock();
        if let Some(tx) = guard.transactions.get_mut(&tx_id) {
            if let Some(last) = tx.entries.last_mut() {
                last.header.crc32 ^= 0xDEAD_BEEF;
            }
        }
    }

    /// Returns telemetry statistics for the journal.
    pub fn stats(&self) -> JournalStats {
        let guard = self.inner.lock();
        let active_txs = guard
            .transactions
            .values()
            .filter(|tx| tx.state == TransactionState::Active)
            .count();
        let committed_pending_checkpoint = guard
            .transactions
            .values()
            .filter(|tx| tx.state == TransactionState::Committed)
            .count();
        JournalStats {
            next_tx_id: guard.next_tx_id,
            active_txs,
            committed_pending_checkpoint,
            total_committed: guard.total_committed,
            total_aborted: guard.total_aborted,
            total_checkpointed: guard.total_checkpointed,
            total_logged_blocks: guard.total_logged_blocks,
            total_replayed_blocks: guard.total_replayed_blocks,
        }
    }
}

/// Global Metadata & Block Write-Ahead Journal instance.
pub static GLOBAL_JOURNAL: Journal = Journal::new();
