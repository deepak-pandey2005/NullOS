# Chapter 29: Dynamic Linking, Shared Libraries & Runtime Relocations
### *ELF64 Dynamic Segments, Global Offset Table (GOT), Procedure Linkage Table (PLT), AMD64 Relocations, Shared Objects (.so), and dlopen/dlsym Runtime Linker*

---

## 1. Evolution of Binary Execution: Static vs. Dynamic Linking

In early computing systems and the initial phases of NullOS, all user executables were **statically linked**. A static linker (`ld`) resolved every function call and global variable reference at build time, binding the entire standard library (`libc.a`) directly into each program file.

While static linking offers simplicity—creating self-contained standalone binaries without runtime external dependencies—it introduces severe real-world scaling penalties:

1. **Storage Bloat**: If 100 system utilities each call `printf()`, `malloc()`, and `strlen()`, identical machine code is duplicated 100 times across the disk.
2. **Physical Memory Waste**: Each static process loads its own copy of the standard library into physical RAM. With dynamic linking, read-only code segments (`.text`, `.rodata`) of shared libraries are mapped once into physical frames and shared across all running processes.
3. **Impossibility of Runtime Patching**: Fixing a bug or security vulnerability in a statically linked function requires recompiling and redistributing every binary on the operating system. With shared libraries, updating `/lib/libc.so` immediately benefits all applications on their next execution.
4. **No Runtime Plugin Architecture**: Static binaries cannot load arbitrary foreign code at runtime. Dynamic linking enables the `dlopen()` / `dlsym()` interface, allowing programs to load plugins, drivers, and game modules on demand.

```
┌────────────────────────────────────────────────────────────────────────┐
│               NullOS Dynamic Linking Subsystem (M34)                   │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│   ┌────────────────────────────────────────────────────────────────┐   │
│   │     Userspace Dynamic APIs: dlopen(), dlsym(), dlclose()       │   │
│   │             (libdl.so / libc.so standard POSIX C ABI)          │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │       Global Offset Table (GOT) & Procedure Linkage (PLT)      │   │
│   │      Position-Independent Code (PIC) & Runtime Relocations     │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │       Dynamic Linker Core: Symbol Resolution & Dependency      │   │
│   │      DAG Traversal (DT_NEEDED, DT_STRTAB, DT_SYMTAB, DT_RELA)  │   │
│   └───────────────────────────────▲────────────────────────────────┘   │
│                                   │                                    │
│   ┌───────────────────────────────┴────────────────────────────────┐   │
│   │   ELF64 Binary Loader: PT_DYNAMIC, PT_INTERP, and ET_DYN       │   │
│   │            (VFS File Resolution from /lib and /disk)           │   │
│   └────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Anatomy of an ELF Shared Object (`.so`)

A shared object is an ELF file with `e_type == ET_DYN` (3). Unlike static executables (`ET_EXEC`), which expect fixed virtual addresses (e.g., `0x400000`), a shared object is compiled as **Position-Independent Code (PIC)** using the GCC flag `-fPIC`.

### 2.1 The Dynamic Program Header (`PT_DYNAMIC`)

When an ELF binary or shared object requires runtime linking, its Program Header Table contains an entry with `p_type == PT_DYNAMIC` (2). This segment points to the `.dynamic` section, which consists of an array of 16-byte `Elf64_Dyn` structures terminated by `DT_NULL`:

```rust
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Dyn {
    pub d_tag: i64,   // Dynamic tag identifier (DT_*)
    pub d_val: u64,   // Integer value or virtual memory pointer
}
```

### 2.2 Key Dynamic Tags

| Tag Constant | Value | Description |
| :--- | :--- | :--- |
| `DT_NULL` | `0` | Marks the end of the `_DYNAMIC` array |
| `DT_NEEDED` | `1` | String table offset of a needed shared library name (e.g. `"libc.so"`) |
| `DT_PLTRELSZ` | `2` | Total byte size of relocations associated with the PLT |
| `DT_PLTGOT` | `3` | Address of the Global Offset Table (GOT) and/or PLT |
| `DT_STRTAB` | `5` | Virtual address of the dynamic string table (`.dynstr`) |
| `DT_SYMTAB` | `6` | Virtual address of the dynamic symbol table (`.dynsym`) |
| `DT_RELA` | `7` | Virtual address of explicit-addend relocation table (`.rela.dyn`) |
| `DT_RELASZ` | `8` | Total byte size of the `DT_RELA` relocation table |
| `DT_RELAENT` | `9` | Byte size of each `Elf64_Rela` entry (24 bytes on x86_64) |
| `DT_STRSZ` | `10` | Total byte size of the dynamic string table |
| `DT_SYMENT` | `11` | Byte size of each `Elf64_Sym` entry (24 bytes on x86_64) |
| `DT_INIT` | `12` | Address of the library's initialization function (`_init`) |
| `DT_FINI` | `13` | Address of the library's termination function (`_fini`) |
| `DT_SONAME` | `14` | String table offset of this shared object's canonical name |
| `DT_JMPREL` | `23` | Virtual address of relocations associated solely with the PLT (`.rela.plt`) |

---

## 3. Position-Independent Code (PIC), GOT, and PLT

Because a shared library may be loaded at an arbitrary base address chosen at runtime, machine code cannot contain absolute 64-bit virtual memory addresses pointing to variables or functions.

### 3.1 The Global Offset Table (GOT)

The **GOT** is a private data table allocated within the writable data segment of each ELF object. Position-independent code references global variables indirectly through the GOT using `%rip`-relative addressing:

```assembly
# Non-PIC Code (Absolute Addressing):
mov 0x601040, %rax        # Hardcoded address; fails if library is relocated

# PIC Code (GOT Indirection):
mov got_var@GOTPCREL(%rip), %rax   # Read pointer from GOT at fixed relative offset
mov (%rax), %eax                   # Dereference pointer to access actual variable
```

During program startup, the dynamic linker patches the GOT entries with the actual resolved virtual addresses of symbols.

### 3.2 The Procedure Linkage Table (PLT)

Function calls to external shared libraries are routed through the **Procedure Linkage Table (PLT)**:

```
User Code                   PLT Stub (.plt)                 GOT Entry (.got.plt)
┌─────────────┐             ┌─────────────────────────┐     ┌───────────────────────┐
│ call puts   │ ──────────> │ jmp *puts@GOTPCREL(%rip)│ ──> │ Target Function Addr  │
└─────────────┘             └─────────────────────────┘     │ (or Linker Resolver)  │
                                                            └───────────────────────┘
```

In eager binding, the dynamic linker resolves all `R_X86_64_JUMP_SLOT` relocations before handing execution to the user program, populating `.got.plt` entries directly with function targets.

---

## 4. AMD64 / x86_64 Relocations

Relocations specify how the dynamic linker modifies pointers in memory when an object is loaded at `base_addr`. On x86_64, relocations use `Elf64_Rela` with an explicit addend:

```rust
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Rela {
    pub r_offset: u64, // Virtual address of the location to be modified
    pub r_info: u64,   // Upper 32 bits = Symbol Index; Lower 32 bits = Relocation Type
    pub r_addend: i64, // Explicit signed addend
}
```

### 4.1 Relocation Types

1. **`R_X86_64_RELATIVE` (Type 8)**:
   $$\text{Target} = B + A$$
   where $B$ is the shared library's load base address and $A$ is `r_addend`. This relocation adjusts internal pointers (such as string literals or static variable addresses) to account for where the library was placed in virtual memory.

2. **`R_X86_64_GLOB_DAT` (Type 6)**:
   $$\text{Target} = S$$
   Sets a GOT entry to the virtual address of the global symbol $S$ resolved from dependency libraries.

3. **`R_X86_64_JUMP_SLOT` (Type 7)**:
   $$\text{Target} = S$$
   Sets a PLT GOT slot to the entry point of the external function $S$.

4. **`R_X86_64_64` (Type 1)**:
   $$\text{Target} = S + A$$
   Direct 64-bit absolute relocation combining resolved symbol address $S$ with explicit addend $A$.

---

## 5. Dynamic Symbol Resolution: `dlopen` and `dlsym`

The standard POSIX runtime API allows applications to load libraries dynamically:

```c
#include <dlfcn.h>

void *handle = dlopen("/lib/libc.so", RTLD_NOW);
if (!handle) {
    printf("Failed to load library: %s\n", dlerror());
    return 1;
}

int (*my_printf)(const char *, ...) = (int (*)(const char *, ...))dlsym(handle, "printf");
my_printf("Hello from dynamically resolved printf via dlsym!\n");

dlclose(handle);
```

### 5.1 Kernel Implementation Architecture

In NullOS, dynamic linking is natively integrated into the kernel's process loader:
- When `exec` executes an ELF file, it checks for `p_type == PT_DYNAMIC` or `e_type == ET_DYN`.
- The linker parses `DT_NEEDED` tags, loading shared libraries from `/lib/` or `/disk/`.
- Symbols are resolved across the global symbol table scope.
- Relocations (`RELATIVE`, `GLOB_DAT`, `JUMP_SLOT`) are applied to user pages before execution begins.
- Shell diagnostics with `ldd <path>` and `libs` provide complete visibility into dependencies and relocation tables.

