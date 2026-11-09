# Chapter 27: Intel High Definition Audio (HDA) & Sound Subsystems
### *Intel HD Audio Architecture, CORB/RIRB Ring Buffers, Codec Nodes & Audio Widgets, Stream Descriptor DMA Engines, PCM Synthesizer, and Tone Playback*

---

## 1. Evolution of PC Audio: From AC'97 to Intel HD Audio

In the early personal computer era, audio generation was restricted to the 1-bit timer-driven PC speaker (Port 0x61). By the late 1990s, the **AC'97 (Audio Codec '97)** standard introduced multi-channel sound over a fixed-rate 48 kHz digital serial bus. However, AC'97 suffered from significant architectural bottlenecks:
1. **Fixed Sample Rate**: Everything had to be hardware-resampled to 48 kHz.
2. **Limited Bandwidth**: A single 12.288 MHz link shared between all input and output channels.
3. **Lack of Modern Widget Hierarchy**: Codecs had fixed register mappings with little runtime flexibility for jack retasking or dynamic multi-streaming.

To eliminate these constraints, Intel introduced the **High Definition Audio (HDA)** specification (originally codenamed *Azalia*). HDA provides:
- **Independent Multi-Streaming**: Multiple simultaneous audio streams (e.g., VoIP headset and 7.1 surround sound playing distinct audio concurrently).
- **High Bandwidth & High Fidelity**: Up to 192 kHz sample rates and 32-bit resolution across 16 channels.
- **Dynamic Topology**: The codec is modeled as a tree of independent **Widgets** (Audio Output Converters, Pin Complexes, Mixers, Amplifiers) navigable and configurable at runtime.
- **Circular DMA Ring Queues**: Communication uses dedicated hardware rings for commands (**CORB**) and responses (**RIRB**), completely avoiding synchronous I/O blocking.

```
┌────────────────────────────────────────────────────────────────────────┐
│                   NullOS Audio Subsystem (M32)                         │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│   ┌────────────────────────────────────────────────────────────────┐   │
│   │           Audio API & Shell Commands (beep, play, audio)       │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │           PCM Tone Generator & Buffer Management               │   │
│   │       (44.1 kHz / 48 kHz, 16-bit Stereo, Ping-Pong DMA)        │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │       HDA Stream DMA Engine (Buffer Descriptor List - BDL)     │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │        CORB / RIRB Command Engine (32-bit Verbs / 64-bit Resp) │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │         PCI Bus: Class 0x04 (Multimedia), Subclass 0x03        │   │
│   │         MMIO BAR0 (Global Control, Streams, CORB/RIRB)         │   │
│   └────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. HDA Controller Architecture & Register Layout

An Intel HDA controller appears on the PCI bus with:
- **Class Code**: `0x04` (Multimedia Device)
- **Subclass Code**: `0x03` (Audio Controller)
- **BAR0**: 64-bit or 32-bit Memory-Mapped I/O (MMIO) region, typically 16 KiB.

### 2.1 Global Registers
All registers are accessed relative to the base of BAR0:

| Offset | Register Name | Size | Description |
| :---: | :--- | :---: | :--- |
| `0x00` | **GCAP** | 16-bit | **Global Capabilities**: `NUM_OSS` (bits 12..15), `NUM_ISS` (bits 8..11), `64OK` (bit 0) |
| `0x02` | **VMIN** | 8-bit | Minor Version number |
| `0x03` | **VMAJ** | 8-bit | Major Version number (`0x01` for HDA 1.0) |
| `0x08` | **GCTL** | 32-bit | **Global Control**: `CRST` (Controller Reset, bit 0) |
| `0x0C` | **WAKEEN** | 16-bit | Wake Enable mask for codec state changes |
| `0x0E` | **STATESTS** | 16-bit | **State Change Status**: Bit $N$ indicates Codec $N$ is connected and active |
| `0x20` | **INTCTL** | 32-bit | Global & Stream Interrupt Control (`GIE` bit 31, `CIE` bit 30) |
| `0x24` | **INTSTS** | 32-bit | Interrupt Status |

### 2.2 Controller Reset Sequence (`CRST`)
Before issuing commands or setting up streams, software must initialize the controller state machine:
1. Clear `GCTL.CRST` (bit 0) to `0`.
2. Spin-poll until hardware confirms `GCTL.CRST == 0` (Controller in reset).
3. Wait at least 100 microseconds for hardware clocks and analog codec state to stabilize.
4. Set `GCTL.CRST` to `1` to release the controller from reset.
5. Spin-poll until hardware asserts `GCTL.CRST == 1`.
6. Wait for codecs to announce presence: read `STATESTS` (bits 0..14). A non-zero bitmask indicates connected codecs.

---

## 3. CORB & RIRB Ring Buffers

Communication between the OS driver and connected codecs is conducted via two DMA ring buffers in main memory:

```
                ┌──────────────────────────────────────────┐
                │          Host Memory (RAM)               │
                ├──────────────────────────────────────────┤
                │                                          │
    CORBWP ────►│  [ Verb 0 ] [ Verb 1 ] [ Verb 2 ] ...    │ (Outbound Verbs)
                │                                          │
                ├──────────────────────────────────────────┤
                │                                          │
    RIRBWP ────►│  [ Resp 0 ] [ Resp 1 ] [ Resp 2 ] ...    │ (Inbound Responses)
                │                                          │
                └──────────────────────────────────────────┘
```

### 3.1 CORB (Command Outbound Ring Buffer)
The CORB carries 32-bit verbs from software to the codecs.
- **`CORBLBASE` / `CORBUBASE` (`0x40`/`0x44`)**: 64-bit physical address of the CORB ring buffer (128-byte aligned).
- **`CORBWP` (`0x48`)**: Software Write Pointer (bits 0..7). Incremented after writing a verb.
- **`CORBRP` (`0x4A`)**: Hardware Read Pointer (bits 0..7). Advanced by controller as it consumes verbs. Bit 15 (`CORBRPRST`) resets the pointer.
- **`CORBCTL` (`0x4C`)**: Control register. Bit 1 (`CORBRUN`) activates the DMA engine.
- **`CORBSIZE` (`0x4E`)**: Ring size selector: `00` = 2 entries (8 bytes), `01` = 16 entries (64 bytes), `10` = 256 entries (1024 bytes).

### 3.2 RIRB (Response Inbound Ring Buffer)
The RIRB carries 64-bit responses from codecs back to software.
- **`RIRBLBASE` / `RIRBUBASE` (`0x50`/`0x54`)**: 64-bit physical address of the RIRB buffer (128-byte aligned).
- **`RIRBWP` (`0x58`)**: Hardware Write Pointer (bits 0..7). Updated by controller when posting responses. Bit 15 (`RIRBWPRST`) resets the pointer.
- **`RIRBCTL` (`0x5C`)**: Bit 1 (`RIRBDMAEN`) enables DMA receipt.
- **`RIRBSIZE` (`0x5E`)**: Ring size selector: `00` = 2 entries, `01` = 16 entries, `10` = 256 entries (2048 bytes).
- Each entry consists of:
  - Lower 32 bits: Response data payload.
  - Upper 32 bits: Response extension (Codec Address in bits 0..3, Unsolicited flag in bit 4).

---

## 4. HDA Verbs & Codec Widget Tree

HDA codecs are modeled as hierarchical trees of functional blocks called **Widgets**:
```
Root Node (NID 0)
└── Audio Function Group (e.g. NID 1)
    ├── Audio Output Converter (DAC, e.g. NID 2)
    │   └── Converts digital PCM audio stream into analog signals
    └── Pin Complex (Audio Jack / Speaker, e.g. NID 4)
        └── Output amplifier and physical 3.5mm jack / speaker driver
```

### 4.1 32-Bit Verb Encoding
Every command sent through the CORB is a 32-bit packet:
```
 31      28 27                  20 19                        0
+----------+----------------------+---------------------------+
| CAD (4b) |      NID (8b)        |      Verb Payload (20b)   |
+----------+----------------------+---------------------------+
```
- **CAD (Codec Address)**: `0` to `14`.
- **NID (Node ID)**: Target widget within the codec (`0` for Root Node).
- **Verb Payload**: 4-bit, 12-bit, or 16-bit command selector plus parameters.

### 4.2 Essential HDA Verbs
- **`Get Parameter` (`0xF00`)**:
  - `0x00`: Vendor ID & Device ID (e.g., `0x10EC` for Realtek).
  - `0x04`: Subordinate Node Count (Total widgets + starting NID).
  - `0x09`: Audio Widget Capabilities (Type: Output DAC = 0, Pin Complex = 4, Mixer = 2).
- **`Set Stream & Channel` (`0x706`)**: Binds an Audio Output DAC to a DMA stream ID (1) and channel (0).
- **`Set Converter Format` (`0x2` / `0xA`)**: Configures sample rate, bit depth, and channel count.
- **`Set Pin Control` (`0x707`)**: Enables output amplifier (`0x40`) and headphone drive (`0x80`).
- **`Set Amplifier Gain / Mute` (`0x3` / `0xB`)**: Unmutes the DAC and Pin Complex (`0xB000`).

---

## 5. Stream Descriptors & Buffer Descriptor Lists (BDL)

Audio data is streamed directly from RAM into the DAC via **Stream Descriptor DMA Engines**:
- Output streams begin at offset `0x80 + NUM_ISS * 0x20`.
- Each stream is governed by a **Buffer Descriptor List (BDL)** table containing entries describing contiguous memory buffers:

```
Buffer Descriptor List (BDL) Entry (16 Bytes):
├── Base Address (64-bit): Physical address of PCM audio buffer
├── Length (32-bit):       Size of buffer in bytes (e.g. 4096)
└── Flags (32-bit):        Bit 0 = Interrupt on Completion (IOC)
```

To achieve tear-free continuous streaming without glitches, the HDA engine operates cyclically across at least two BDL entries (**ping-pong buffers**). While the DMA engine reads buffer 0, software prepares buffer 1.

### Stream Registers:
- **`SD_CTL` (24-bit)**: Stream ID in bits 20..23, Run flag (`RUN`) in bit 1.
- **`SD_FMT` (16-bit)**: PCM audio encoding format:
  - Bit 14: Base rate (0 = 48 kHz, 1 = 44.1 kHz).
  - Bits 4..6: Bits per sample (001 = 16-bit PCM).
  - Bits 0..3: Number of channels minus 1 (0 = Mono, 1 = Stereo).
- **`SD_BDLPL` / `SD_BDLPU`**: 64-bit physical pointer to the BDL array.
- **`SD_CBL` (32-bit)**: Cyclic Buffer Length (sum of all buffer lengths).
- **`SD_LVI` (16-bit)**: Last Valid Index (e.g., 1 for a 2-buffer ping-pong list).
- **`SD_LPIB` (32-bit)**: Link Position in Buffer (current byte read offset).

---

## 6. Procedural PCM Sound Synthesis

NullOS implements a mathematical sound synthesizer that renders audio directly into PCM DMA memory buffers:
For a sample at index $t$ under sample rate $f_s$ (e.g., 44,100 Hz) producing frequency $f$:
$$\text{phase} = \frac{2\pi \cdot f \cdot t}{f_s}$$
$$\text{sample}(t) = \lfloor A \cdot \sin(\text{phase}) \rfloor$$
For 16-bit signed audio, amplitude $A = 16,000$ (comfortably below maximum $32,767$ to avoid digital clipping). Each stereo frame consists of two consecutive 16-bit integers: `[Left, Right]`.

