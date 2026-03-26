# Stucker

> [!NOTE]
> And when Alexander saw the breadth of his dominion, he wept for there were no more stacks to fuck.

You've probably been told that the stack is only for fixed-size data and the heap must be used for dynamically-sized data. This isn't exactly true... 

I present to you, a language that implements it's own custom data-structure in stack memory that allows you to resize variables on the stack, without any heap allocations!

You can see the accompanying blog post [here](https://ogghostjelly.github.io/slog/alloca2/index.html).

> [!WARNING]
> This is a toy language. So many segfaults...

```
void double_size(&[]i32 arr, u64 size) {
    set *arr = [size * 2u64]i32;
}

i32 main() {
    # init array with 3 items
    def u64 size = 3u64;
    def []i32 arr = [size]i32;
    
    double_size(&arr, size);

    set *(&arr)[4u16] = 3;

    return 0;
}
```

This isn't the same as [`alloca`](https://www.man7.org/linux/man-pages/man3/alloca.3.html) or [VLA](https://en.wikipedia.org/wiki/Variable-length_array)s because you can resize an existing allocation! You can find a working hashmap implementation without any heap allocations in the [`examples/`](./examples/) directory.

The compiler outputs NASM assembly. You'll need to use `nasm` to build executable binaries. It's only been tested on x86-64 Linux. Any other OS or architecture isn't guaranteed to work. (and likely won't)

# Implementation

## Stack

Each stack item is stored as some data followed by a size tag.

```
<DATA> <SIZE>
```

If we want to allocate the number `3` which is a `u32` the compiler would generate the following assembly.
```
; allocate <DATA>
sub rsp, 4 ; allocate 4-bytes for u32
mov dword [rsp], 3 ; set the number to 3

; allocate <SIZE>
sub rsp, 8 ; all size tags are an 8-byte u64
mov [rsp], 12 ; the total size is 12 (8+4)
```
Note how the size tag stores the size of the data AND an extra 8-bytes for the tag itself.

In-memory this would look like:
```
<3> <12>
        |
       rsp
```

# Indexes vs References vs Addresses

I use a lot of words to refer to "thing that points to the stack" and every word means something slightly different. I'll define all the terms I use here:

- Indexes (specifically the `Index` struct) is what the compiler uses at compile-time to uniquely identify values in the stack. You can think of it like a path to some data.
- References are stable pointers to values in the stack. They are still valid even when items before it are resized. They are explained in the [section below](#references-). It's basically just an `Index` but stored at runtime instead of compile-time.
- Addresses are the raw unstable memory addresses to values in the stack. They (usually) aren't accessible in the language, which means you can't actually get the raw address of a stack item. It's more of a compiler detail, like when you want to read a memory location you'll need to use the `idx2addr` function that generates the right assembly to convert a compile-time `Index` into an address that can be utilized at runtime.

A stable pointer means it will always point to that object and will only be invalidated if that object is deallocated. An unstable pointer may be invalidated if a stack item is resized, causing the stack to shift around.

## References (`&`)

If you imagine the stack as an array. References are an index into that array.

```
<DATA> <DATA> <DATA>
                    |
                   rsp
```

We can convert a reference into an address by calculating how many steps to jump back from rsp with `length - index - 1`. For example, if we have a stack of size `3` and we want to get the value at a reference of `0` then we would jump `2` steps back from rsp to reach our data. The length of the stack is stored in the `rbp` register.

Jump back `2` steps:
```
<DATA> <DATA> <DATA>
      |
     rsp
```

We can store references to nested data by storing multiple indexes. For example, if we want to make a reference to the data at `&stack[3][2][6]` we'd store 3 `u16`s (`3`, `2` and `6`) to represent all the indexes. That means deeply nested structures will have very long references, and because they are stored as `u16` you can only have up to `65536` stack items or else the reference will overflow.

# Notes

## String Types
Static strings are `u64` types. 64-bit pointers into program memory. There are no dynamic string types but you might be able to make your own.

## Buffered Print

```
extern "C" void printf(u64 fmt);

int main() {
    printf("Hello, World");
}
```
This won't print any output because `printf` is buffered and stucker doesn't automatically flush buffers when the program ends. You need to add a `\n` or manually flush stdout with something like `fflush`.

## Function Prototypes

Define function prototypes:
```
void a();
void b();
```
Then give them a body:
```
void a() {
    b();
}

void b() {
    a();
}
```

## Looping
There are currently no `continue` or `break` statements.

Sum of `0..9`:
```
def i32 count = 0;
for (def i32 i = 0; i < 10; set i = i + 1) {
    set count = count + i;
}
return count;
```

Hang forever:
```
while (1) {}
```

## Casting

The `as` keyword is comparable to C++'s `reinterpret_cast`. It is a NOP that unsafely converts the type of an object into another. If the object is a number it's bits will be zero-extended or truncated without preserving sign bits.
```
def i32 x = as(i32)x;
```

## Structs

Make a recursive struct:
```
struct MyStruct {
    i32 a,
    u8 b,
    &MyStruct c,
}

i32 main() {
    def MyStruct x;
    set x.a = 1;
    set x.b = 2u8;
    set *x.c.a = 3;
    set *x.c.b = 4u8;
    return 0;
}
```

Crash the compiler because the struct is infinitely sized:
```
struct MyStruct {
    i32 a,
    u8 b,
    MyStruct c,
}

i32 main() {
    def MyStruct x;
    return 0;
}
```

## Array Initialization and Indexing

Array initialization uses `u64`:
```
# using u64 to create an array with 3 elements
#                 vvv
def []i32 arr = [3u64]i32;
```
Array indexing uses `u16`:
```
# using u16 to get the first element of an array
#                      vvv
def &i32 arr = (&arr)[0u16];
```

## Resizing
```
def i32 x = 0;
```
Here is how that is represented on the stack:
```
<DATA> <SIZE>
<0>    <4>
```
And when we resize it...
```
resize(&x, 8);
```
A new size tag gets inserted.
```
<--DATA-->    <SIZE>
<0>    <4>    <8>
```
So this program will exit with the exit code `4`:
```
i32 main() {
    def i32 x = 0;
    resize(&x, 8);
    return x;
}
```

Structs are the same way.
```
struct MyStruct {
    i32 first,
    i32 second,
    i32 third,
}

def MyStruct my_struct;
my_struct.first = 1;
my_struct.second = 2;
my_struct.third = 3;
```
They are stored like so:
```
<------DATA----->    <SIZE>
<3>    <2>    <1>    <12>
```
And if we resize...
```
resize(&my_struct, 16);
```
```
<----------DATA--------->    <SIZE>
<3>    <2>    <1>    <12>    <16>
```
This program will exit with the exit code `12`:
```
struct MyStruct {
    i32 first,
    i32 second,
    i32 third,
}

i32 main() {
    def MyStruct my_struct;
    
    set my_struct.first = 1;
    set my_struct.second = 2;
    set my_struct.third = 3;

    resize(&my_struct, 16);

    return my_struct.first;
}
```

## Resizing values inside containers is broken

The following code will segfault.
```
struct MyContainer {
    i32 value,
}

i32 main() {
    def MyContainer x;
    set x.value = 3;

    resize(&x.value, 8);

    return 0;
}
```

You cannot resize data that is inside an array or inside a struct. This is because I'm lazy and can't be bothered to implement this functionality. I've spent too long on this dumbass project already ;-; let me [bee free](./media/bee.jpg)

## INT3

You can manually insert `int3` NASM intructions with the `breakpoint` keyword.
```
i32 main(){
    breakpoint;
    return 0;
}
```