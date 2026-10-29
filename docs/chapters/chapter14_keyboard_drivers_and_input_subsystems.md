# Chapter 14: Keyboard Drivers & Input Subsystems — The Bridge Between Human & Machine

> *"When a human presses a key on a keyboard, the CPU does not receive the letter 'A'. It receives a raw, silent pulse of electrical current—a scan code. The operating system's job is to translate that mechanical switch into human thought."*

---

## 1. The Human-Computer Illusion

When you press the key labeled **`A`** on your keyboard while using an operating system, what happens?

To a beginner, it feels like the keyboard sends the ASCII byte `0x41` ('A') down the wire directly into the running program. But hardware does not work that way. In physical electronics:

1. Underneath each plastic keycap lies a mechanical switch or a membrane capacitive pad.
2. Inside the keyboard is a tiny, dedicated 8-bit microcontroller (historically an Intel 8048).
3. The microcontroller continuously scans an $(X, Y)$ matrix of wire rows and columns.
4. When a circuit closes (a key is pushed down), the microcontroller detects the switch closure and sends an arbitrary hardware identifier called a **Make Code**.
5. When the user lifts their finger and the spring pushes the key back up, the circuit breaks. The microcontroller detects this and sends a **Break Code**.

Neither of these numbers is ASCII! On an IBM-compatible PC, pressing the **`A`** key generates the hexadecimal number `0x1E`. Releasing the **`A`** key generates `0x9E`.

If an operating system does not possess a **Keyboard Driver**, every single keystroke on the physical machine is completely ignored. The monitor will sit motionless, displaying a shell prompt that can never be answered.

In this chapter, we will build a complete **Intel 8042 PS/2 Keyboard Driver** from scratch in `#![no_std]` Rust, understand the IBM PC Scan Code Set 1 standard, build a modifier state machine (Shift, CapsLock, Ctrl, Alt), and connect it directly to the NullOS Interactive Shell.

---

## 2. The Intel 8042 PS/2 Controller Architecture

Even on modern high-end gaming rigs and ultrabooks running USB or Bluetooth keyboards, the motherboard chipset's xHCI USB controller or UEFI firmware provides **PS/2 Legacy Emulation**. This ensures that before USB drivers load, the keyboard behaves identically to the classic **Intel 8042 Keyboard Controller** introduced in the 1984 IBM PC/AT.

The 8042 controller communicates with the x86_64 CPU through two standardized legacy **I/O Ports**:

| I/O Port | Direction | Register Name | Description |
| :--- | :--- | :--- | :--- |
| **`0x60`** | Read | **Data Port (Output Buffer)** | Reads the latest scan code received from the keyboard. |
| **`0x60`** | Write | **Data Port (Input Buffer)** | Sends configuration commands or LED controls to the keyboard. |
| **`0x64`** | Read | **Status Register** | Reports whether data is ready to be read, or if the controller is busy. |
| **`0x64`** | Write | **Command Register** | Sends control instructions directly to the 8042 controller itself. |

```
                       +---------------------------------------+
                       |   Physical Keyboard (or QEMU GUI)     |
                       +---------------------------------------+
                                           |
                                  Raw Scan Code Byte
                                           v
+------------------------------------------------------------------------------+
|                     Intel 8042 PS/2 Controller Chipset                       |
|                                                                              |
|   +--------------------------+               +--------------------------+    |
|   |   Status Register (0x64) |               |     Data Port (0x60)     |    |
|   |                          |               |                          |    |
|   |  Bit 0: Output Full (1)  | ------------> | Holds unread scan code   |    |
|   |  Bit 1: Input Full (0)   |               | byte (e.g. 0x1E for 'A') |    |
|   +--------------------------+               +--------------------------+    |
+------------------------------------------------------------------------------+
                                           |
                              x86_64 inb(0x60) / inb(0x64)
                                           v
+------------------------------------------------------------------------------+
|                          NullOS Keyboard Driver                              |
|                                                                              |
|   1. Poll Status Port 0x64 (Check Bit 0 != 0)                                |
|   2. Read Data Port 0x60 via inb instruction                                 |
|   3. Feed byte into Scan Code Set 1 State Machine                            |
|   4. Update Modifiers (Shift = true, CapsLock toggle)                        |
|   5. Output Decoded ASCII Char ('A') -> Feed into Shell & Display            |
+------------------------------------------------------------------------------+
```

### The 8042 Status Register (Port `0x64`)

Before reading from Port `0x60`, the OS **must** check Port `0x64`. If you attempt to read Port `0x60` when no key has been pressed, you will read stale garbage or trigger undefined bus cycles.

The 8 bits of the Status Register tell us the exact electrical condition of the controller:

- **Bit 0 (`0x01`) — Output Buffer Status**:
  - `0`: Output buffer is empty. No key has been pressed.
  - `1`: Output buffer is FULL. A scan code from the keyboard is waiting at Port `0x60`!
- **Bit 1 (`0x02`) — Input Buffer Status**:
  - `0`: Input buffer is empty. The CPU may safely write commands to Port `0x60` or `0x64`.
  - `1`: Input buffer is full. The controller is still processing the previous command; do not write yet.
- **Bit 2 (`0x04`) — System Flag**: Set to 1 if the system passed power-on self-test.
- **Bit 3 (`0x08`) — Command/Data**: `0` if last write to Port `0x60` was data; `1` if write to `0x64` was a command.
- **Bit 4 (`0x10`) — Keyboard Inhibit**: `0` if keyboard is locked; `1` if keyboard is active and unlocked.
- **Bit 5 (`0x20`) — Auxiliary Output Buffer Full**: `1` if the data at Port `0x60` is from the PS/2 Mouse rather than the keyboard.
- **Bit 6 (`0x40`) — Time-out Error**: Set to 1 if communication timed out.
- **Bit 7 (`0x80`) — Parity Error**: Set to 1 if electrical transmission suffered a bit error.

---

## 3. Scan Code Set 1: Decoding the Mechanical Switches

Historically, keyboards have supported three sets of scan codes:
- **Scan Code Set 1**: Introduced by the original IBM PC XT. Simple, universal, and the default mode emulated by QEMU, VirtualBox, VMware, and modern UEFI firmware.
- **Scan Code Set 2**: The native protocol of PS/2 keyboards. Usually translated back into Set 1 by the 8042 controller chipset automatically.
- **Scan Code Set 3**: An obscure set designed for IBM 3270 terminal emulation, rarely used today.

In NullOS, we implement **Scan Code Set 1**.

### Make Codes vs. Break Codes

In Scan Code Set 1, every physical key on the keyboard has two codes:

1. **Make Code (Key Press)**: A byte between `0x01` and `0x58`.
2. **Break Code (Key Release)**: The exact same Make Code, but with **Bit 7 (the high bit, `0x80`) set to 1**!

$$\text{Break Code} = \text{Make Code} \mid 0x80$$

For example:
- Pressing **`A`**: Make code = `0x1E` (`0001 1110b`).
- Releasing **`A`**: Break code = `0x1E | 0x80` = `0x9E` (`1001 1110b`).
- Pressing **Left Shift**: Make code = `0x2A`.
- Releasing **Left Shift**: Break code = `0xAA`.

This elegant bit-flip makes detecting releases trivial in code:
```rust
if (scancode & 0x80) != 0 {
    // High bit is set: this is a KEY RELEASE event!
    let make_code = scancode & 0x7F;
    handle_key_release(make_code);
} else {
    // High bit is clear: this is a KEY PRESS event!
    handle_key_press(scancode);
}
```

### The Scan Code Set 1 Lookup Table

Here is the standard mapping of common Scan Code Set 1 make codes to their unshifted and shifted ASCII counterparts:

| Scan Code (Hex) | Key Name | Normal ASCII | Shifted ASCII |
| :--- | :--- | :--- | :--- |
| `0x01` | Escape | `\x1B` (ESC) | `\x1B` |
| `0x02` | `1` | `'1'` | `'!'` |
| `0x03` | `2` | `'2'` | `'@'` |
| `0x04` | `3` | `'3'` | `'#'` |
| `0x05` | `4` | `'4'` | `'$'` |
| `0x06` | `5` | `'5'` | `'%'` |
| `0x07` | `6` | `'6'` | `'^'` |
| `0x08` | `7` | `'7'` | `'&'` |
| `0x09` | `8` | `'8'` | `'*'` |
| `0x0A` | `9` | `'9'` | `'('` |
| `0x0B` | `0` | `'0'` | `')'` |
| `0x0C` | `-` | `'-'` | `'_'` |
| `0x0D` | `=` | `'='` | `'+'` |
| `0x0E` | Backspace | `\x08` (BS) | `\x08` |
| `0x0F` | Tab | `\t` | `\t` |
| `0x10`..`0x19`| `Q` through `P` | `'q'`..`'p'` | `'Q'`..`'P'` |
| `0x1A` | `[` | `'['` | `'{'` |
| `0x1B` | `]` | `']'` | `'}'` |
| `0x1C` | Enter | `\n` | `\n` |
| `0x1D` | Left Control | *(Modifier)* | *(Modifier)* |
| `0x1E`..`0x26`| `A` through `L` | `'a'`..`'l'` | `'A'`..`'L'` |
| `0x27` | `;` | `';'` | `':'` |
| `0x28` | `'` | `'\''` | `'"'` |
| `0x29` | `` ` `` | `` '`' `` | `'~'` |
| `0x2A` | Left Shift | *(Modifier)* | *(Modifier)* |
| `0x2B` | `\` | `'\\'` | `'|'` |
| `0x2C`..`0x32`| `Z` through `M` | `'z'`..`'m'` | `'Z'`..`'M'` |
| `0x33` | `,` | `','` | `'<'` |
| `0x34` | `.` | `'.'` | `'>'` |
| `0x35` | `/` | `'/'` | `'?'` |
| `0x36` | Right Shift | *(Modifier)* | *(Modifier)* |
| `0x38` | Left Alt | *(Modifier)* | *(Modifier)* |
| `0x39` | Space | `' '` | `' '` |
| `0x3A` | Caps Lock | *(Toggle)* | *(Toggle)* |

---

## 4. The Modifier State Machine

Notice something critical: **Shift and CapsLock do not behave the same way.**

### Shift vs. CapsLock Rules
1. **Letters (`a`..`z`)**:
   - If Shift is held DOWN and CapsLock is OFF: Output **UPPERCASE** (`A`).
   - If Shift is NOT held and CapsLock is ON: Output **UPPERCASE** (`A`).
   - If Shift is held DOWN and CapsLock is ON: The two cancel out! Output **LOWERCASE** (`a`).
   
   This is the fundamental Boolean **Exclusive-OR (XOR)** property:
   $$\text{Uppercase} = \text{Shift} \oplus \text{CapsLock}$$

2. **Numbers and Symbols (`0`..`9`, `,`, `.`, `/`, etc.)**:
   - CapsLock has **NO EFFECT** on numbers! Pressing `'1'` with CapsLock on must still produce `'1'`, NOT `'!'`.
   - Only the physical Shift key switches numbers to symbols.

### Managing Extended Keys (`0xE0` Prefix)

Some keys on standard 104-key keyboards (like the arrow keys, Right Control, Right Alt, and Delete) did not exist on the original 83-key IBM PC XT keyboard. To maintain backwards compatibility without colliding with existing scan codes, IBM gave them an **escape prefix byte: `0xE0`**.

When you press the Right Alt key, the keyboard does not send one byte. It sends **two bytes**:
1. First byte: `0xE0`
2. Second byte: `0x38`

When our driver encounters `0xE0`, it sets an internal boolean `extended = true` and waits for the next byte to complete the sequence.

---

## 5. Implementation in NullOS

Let us study the complete driver in [kernel/src/drivers/keyboard.rs](../../kernel/src/drivers/keyboard.rs).

### Step 1: Low-Level Port I/O
First, we define hardware port read/write primitives using the x86 `in` and `out` assembly instructions:

```rust
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack),
        );
    }
    value
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack),
        );
    }
}
```

### Step 2: The State Machine
Next, we maintain the keyboard modifier states inside a thread-safe `Spinlock<KeyboardState>`:

```rust
pub struct KeyboardState {
    pub shift: bool,
    pub caps_lock: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub extended: bool,
}

impl KeyboardState {
    pub fn process_scancode(&mut self, scancode: u8) -> Option<char> {
        if scancode == 0xE0 {
            self.extended = true;
            return None;
        }

        if self.extended {
            self.extended = false;
            match scancode {
                0x1D => { self.ctrl = true; return None; }
                0x9D => { self.ctrl = false; return None; }
                0x38 => { self.alt = true; return None; }
                0xB8 => { self.alt = false; return None; }
                _ => return None,
            }
        }

        // Check for Break code (Key Release)
        if (scancode & 0x80) != 0 {
            let make_code = scancode & 0x7F;
            match make_code {
                0x2A | 0x36 => self.shift = false,
                0x1D => self.ctrl = false,
                0x38 => self.alt = false,
                _ => {}
            }
            return None;
        }

        // Handle Make codes (Key Press)
        match scancode {
            0x2A | 0x36 => { self.shift = true; None }
            0x1D => { self.ctrl = true; None }
            0x38 => { self.alt = true; None }
            0x3A => { self.caps_lock = !self.caps_lock; None }
            0x1C => Some('\n'),
            0x0E => Some('\x08'),
            0x0F => Some('\t'),
            0x01 => Some('\x1B'),
            0x39 => Some(' '),
            code => self.translate_printable(code),
        }
    }
}
```

### Step 3: Non-Blocking Polling API
We provide an ergonomic, non-blocking API: `read_char() -> Option<char>`.
It inspects Bit 0 of Status Port `0x64`. If data is waiting, it reads Port `0x60`, decodes the scan code, and returns the character:

```rust
pub fn read_char() -> Option<char> {
    while let Some(scancode) = read_scancode() {
        if let Some(ch) = KEYBOARD.lock().process_scancode(scancode) {
            return Some(ch);
        }
    }
    None
}
```

---

## 6. Unifying Keyboard & Serial Input in the Shell

Now comes the crucial integration. Why was typing broken in the QEMU window initially?

When you launch QEMU:
- Keystrokes inside the **terminal emulator** running QEMU go to **COM1 serial** (`0x3F8`).
- Keystrokes inside the **graphical window** (and physical laptops) go to the **8042 PS/2 controller** (`0x60`).

In [kernel/src/task/shell.rs](../../kernel/src/task/shell.rs), we unified both into a single input stream:

```rust
// In interactive_loop():
let incoming_char = if let Some(ch) = crate::drivers::keyboard::read_char() {
    Some(ch)
} else if let Some(byte) = serial::read_byte() {
    match byte {
        b'\r' | b'\n' => Some('\n'),
        0x08 | 0x7F => Some('\x08'),
        b if b >= 0x20 && b <= 0x7E => Some(b as char),
        _ => None,
    }
} else {
    None
};

if let Some(ch) = incoming_char {
    match ch {
        '\n' => {
            println!();
            execute_command(&line_buf);
            line_buf.clear();
            print!("nullos> ");
        }
        '\x08' => {
            if !line_buf.is_empty() {
                line_buf.pop();
                print!("\x08 \x08"); // Erase character on screen
            }
        }
        c if c >= ' ' && c <= '~' => {
            line_buf.push(c);
            print!("{}", c);
        }
        _ => {}
    }
} else {
    crate::task::yield_now(); // Yield CPU time to background threads
}
```

Now, regardless of whether you are sitting in front of a real physical laptop, typing in the QEMU GUI window, or remoting in over a serial port wire, every single keystroke works flawlessly.

---

## 7. Self-Test Verification

During kernel boot, before the shell starts, `drivers::keyboard::run_self_tests()` verifies that:
1. Normal characters decode to lowercase (`0x1E` $\rightarrow$ `'a'`).
2. Holding Left Shift transforms characters to uppercase (`0x2A` followed by `0x1E` $\rightarrow$ `'A'`).
3. Releasing Left Shift restores lowercase (`0xAA` followed by `0x1E` $\rightarrow$ `'a'`).
4. Toggling CapsLock works as expected (`0x3A` toggles state).
5. Number row punctuation correctly triggers under Shift (`0x02` produces `'1'`, with Shift produces `'!'`).
6. Control keys (`Enter`, `Backspace`, `Space`, `Tab`, `Escape`) produce valid ASCII control bytes.

```text
----------------------------------------
 [KEYBOARD] Initializing PS/2 Controller
----------------------------------------
[KEYBOARD] Intel 8042 PS/2 Keyboard Driver active (Scan Code Set 1).
[KEYBOARD] Running PS/2 keyboard verification self-tests...
[KEYBOARD] All PS/2 keyboard self-tests passed successfully!
```

---

## 8. Summary & Next Steps

We now understand:
1. How mechanical switches create make and break codes rather than ASCII characters.
2. How the Intel 8042 PS/2 controller exposes I/O ports `0x60` and `0x64`.
3. How Scan Code Set 1 encodes key presses and releases using Bit 7.
4. How the XOR rule ($\text{Shift} \oplus \text{CapsLock}$) drives uppercase letter generation.
5. How unifying PS/2 keyboard input and COM1 serial input makes the operating system responsive across all display environments.

Next, in **Chapter 15**, we will connect high-speed persistent block storage via the **VirtIO Block Subsystem**!
