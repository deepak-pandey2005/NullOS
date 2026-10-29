# Chapter 7: Dynamic Kernel Heap Management
### *From Raw Pages to `Box`, `Vec`, and `String`*

In Chapter 5, we built the **Physical Memory Manager** to allocate 4 KiB frames. In Chapter 6, we built **4-Level Paging** to map those frames into virtual memory.

However, operating systems rarely work exclusively in 4 KiB units:
- What if a driver needs a 64-byte struct for a network packet?
- What if a thread needs a dynamically growing `Vec<ThreadId>`?
- What if the kernel wants to format a string with `format!("PID: {}", pid)`?

Allocating an entire 4,096-byte page for a 30-byte string would waste 99.3% of your RAM!

In this chapter, we explore **Layer 3** of the memory hierarchy: the **Kernel Heap Allocator**, Rust's `GlobalAlloc` interface, and how NullOS dynamically grows its heap on demand.

---

## 1. Stack vs. Heap: The Memory Division

Every program has two primary memory regions:

```
+-------------------------------------------------------------+
| STACK MEMORY                                                |
| - Fast LIFO (Last-In, First-Out) managed by RSP             |
| - Allocated and deallocated automatically by function calls |
| - Size MUST be known at compile-time                        |
| - Cannot return pointers to local stack variables!          |
+-------------------------------------------------------------+
                               |
                               v
+-------------------------------------------------------------+
| HEAP MEMORY                                                 |
| - Arbitrary byte-size allocations (12 bytes, 500 KB, etc.)  |
| - Lifespan is dynamic (lives until explicitly freed)        |
| - Backed by virtual memory pages managed by the OS heap     |
+-------------------------------------------------------------+
```

In a `#![no_std]` bare-metal kernel, Rust disables `Box`, `Vec`, and `String` by default because there is no underlying operating system or standard library to provide `malloc()` and `free()`.
**We must build `malloc()` and `free()` ourselves!**

---

## 2. Rust's `GlobalAlloc` Trait

Rust provides a standard interface for memory allocators in `core::alloc::GlobalAlloc`:

```rust
pub unsafe trait GlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout);
}
```

A `Layout` contains two values:
- `size`: Number of bytes requested (e.g. 48 bytes).
- `align`: Memory alignment requirement (e.g. must be an address divisible by 8 or 16).

Once an operating system defines a struct implementing `GlobalAlloc` and tags it with `#[global_allocator]`, the Rust compiler automatically unlocks the `alloc` crate, enabling `Box<T>`, `Vec<T>`, `String`, `BTreeMap`, and `Arc<T>` across the entire kernel!

---

## 3. Allocation Strategies: Bump, Linked List, and Slab

How does an allocator manage chunks of memory?

### Strategy 1: The Bump Allocator (Watermark)
The simplest possible allocator:
- Starts at `heap_start` and keeps a single `next` pointer.
- When an allocation of size $N$ is requested, it rounds `next` up to alignment, returns `next`, and sets `next = next + N`.
- **Pros**: Blazing fast ($O(1)$, just an addition instruction).
- **Cons**: **Cannot free individual allocations!** Memory can only be reclaimed all at once by resetting `next` to zero.

### Strategy 2: The Linked-List Allocator (Free-List)
Used by NullOS ([kernel/src/memory/heap.rs](../../kernel/src/memory/heap.rs)):
- When a memory block is freed, the allocator writes a small header into the freed memory block itself:
  ```rust
  struct FreeBlockNode {
      size: usize,
      next: Option<&'static mut FreeBlockNode>,
  }
  ```
- All free memory blocks form a linked list.
- When `alloc(size)` is called, it searches the linked list for a block large enough (**First-Fit** or **Best-Fit**), carves out the requested bytes, and leaves the remainder in the free list.
- When `dealloc(ptr)` is called, it inserts the block back into the free list and merges (**coalesces**) adjacent free blocks to prevent fragmentation!

### Strategy 3: Slab Allocators (Object Caches)
For frequently created fixed-size kernel structs (like `Thread` control blocks or `File` handles):
- The allocator maintains pre-divided slabs of identical size (e.g., 32-byte slab, 64-byte slab, 256-byte slab).
- Allocation is an instant $O(1)$ pop from a free array, with zero internal fragmentation.

---

## 4. The NullOS Expandable Heap Architecture

In NullOS, we place the Kernel Heap at a dedicated virtual address region:
$$\text{Heap Start Virtual Address} = \mathbf{0x4444\_4444\_0000}$$

### Initial Heap Setup:
1. During boot, `heap::init()` runs.
2. It requests 256 physical frames (1 MiB) from the PMM.
3. It maps those 256 frames into virtual addresses `0x4444_4444_0000` through `0x4444_4454_0000` with `PRESENT | WRITABLE | NO_EXECUTE`.
4. It initializes the linked-list allocator managing that 1 MiB buffer.

```
0x444444440000                                               0x444444540000
+--------------------------------------------------------------------------+
|                        Initial 1 MiB Heap Buffer                         |
|  [Used: Box<Thread>] [Free Block] [Used: Vec<u8>] [Large Free Block...]  |
+--------------------------------------------------------------------------+
```

---

### What Happens When the Heap Runs Out of Memory?
In older toy operating systems, if the heap ran out of space, `alloc()` returned null and the kernel panicked.

In NullOS, our heap is **Dynamically Expandable**!
When an allocation request exceeds available free blocks:
1. The allocator determines how many bytes are needed.
2. It rounds up to the nearest multiple of 4 KiB pages ($N$ pages).
3. It requests $N$ new physical frames from the **Physical Memory Manager**.
4. It maps these new frames directly onto the end of the existing heap (`current_heap_end` $\rightarrow$ `new_heap_end`).
5. It adds the newly mapped memory into the linked-list free pool.
6. The allocation succeeds seamlessly!

```text
[HEAP] Mapping 256 heap pages (1024 KiB) at virtual 0x444444440000...
[HEAP] Kernel Heap active at 0x444444440000 - 0x444444540000 (1 MiB)
...
[HEAP] Dynamic expansion: Added 600 KiB (150 frames). New heap limit: 0x4444445d6000
```

---

## 5. Understanding Memory Fragmentation

Fragmentation is the primary enemy of memory allocators:

```
Initial Free Buffer (100 KB):
[==============================================================================]

Allocate A (20 KB), B (20 KB), C (20 KB):
[==== A (20K) ====][==== B (20K) ====][==== C (20K) ====][==== Free (40K) =====]

Free A and C:
[==== Free (20K) =][==== B (20K) ====][==== Free (20K) =][==== Free (40K) =====]
```

Notice what happened:
- We have $20\text{ KB} + 20\text{ KB} + 40\text{ KB} = \mathbf{80\text{ KB}}$ of total free memory!
- But if a program asks to allocate **50 KB**, the allocation will **FAIL** because no single contiguous block is 50 KB long!

This is called **External Fragmentation**.
NullOS mitigates this by **coalescing**: whenever a block is freed, the allocator checks if the adjacent memory blocks in physical address order are also free. If so, it merges them into a single larger block.

---

## 🧠 Self-Test & Review Questions

1. **Why does Rust require the `GlobalAlloc` trait methods (`alloc` and `dealloc`) to be marked `unsafe`?**
   <details>
   <summary>Click to view answer</summary>
   Memory allocators handle raw raw pointers (<code>*mut u8</code>). The compiler cannot guarantee at compile time that the caller won't use memory after freeing it (use-after-free), double-free memory, or pass an incorrect <code>Layout</code> size during deallocation. The caller must guarantee these invariants.
   </details>

2. **What is the difference between Internal and External Fragmentation?**
   <details>
   <summary>Click to view answer</summary>
   <strong>Internal Fragmentation</strong> occurs when memory is allocated in fixed-size blocks (e.g. allocating a 4,096-byte page for a 100-byte string; the unused 3,996 bytes inside the block are wasted). <strong>External Fragmentation</strong> occurs when free memory is split into many tiny, non-contiguous holes between active allocations, preventing a large contiguous allocation from succeeding.
   </details>

3. **How does NullOS ensure that data stored on the heap cannot be executed as code by a hacker?**
   <details>
   <summary>Click to view answer</summary>
   When mapping virtual memory pages for the kernel heap, NullOS sets the <code>NO_EXECUTE</code> (NX) flag (Bit 63) in the Page Table Entry. If any bug or malicious attack attempts to redirect the CPU's Instruction Pointer (<code>%rip</code>) to execute code located in the heap, the CPU hardware immediately raises a Page Fault (#PF) exception.
   </details>

---

*In [Chapter 8: Multitasking, Context Switching & Schedulers](chapter08_multitasking_and_scheduler.md), we explore how the kernel creates threads, swaps CPU registers in assembly, and preemptively schedules tasks using the APIC timer.*
