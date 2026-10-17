use uefi::system::with_config_table;
use uefi::table::cfg::{ACPI_GUID, ACPI2_GUID};

pub fn find_acpi_rsdp() -> Option<u64> {
    with_config_table(|entries| {
        // Look for ACPI 2.0 first
        for entry in entries {
            if entry.guid == ACPI2_GUID {
                return Some(entry.address as u64);
            }
        }
        // Fallback to ACPI 1.0
        for entry in entries {
            if entry.guid == ACPI_GUID {
                return Some(entry.address as u64);
            }
        }
        None
    })
}
