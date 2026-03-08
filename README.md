# Stucker

Stack fucker.

> [!NOTE]
> Stack memory is just memory, you can treat it how you want.

A compiled programming language that implements it's own custom data-structure in stack memory that allows you to resize variables on the stack, without any heap allocations.

```
i32 main() {
    # create a list with 3 items
    def []i32 x = [3]i32;

    # put some numbers in the list
    # (numbers default to `i32` if not given a specific type)
    set x[0u64] = 1;
    set x[1u64] = 2;
    set x[2u64] = 3;

    ; resize the list to 6 elements
    ; an i32 is 4-bytes so we need to do `4*6`
    resize(&x, 4 * 6);

    ; put some more items in the list
    set x[3u64] = 1;
    set x[4u64] = 2;
    set x[5u64] = 3;

    # return an exit code
    return 0;
}
```

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

Of course, the compiler wouldn't literally generate that assembly since it's more efficient to do:
```
sub rsp, 12
mov dword [rsp+8], 3
mov qword [rsp], 12
```

## References (`&`)

If you imagine the stack as an array. References are an index into that array. For example, if we have a stack of size `3` and we want to get the value at a reference of `0` then we would jump `2` steps back, from rsp, to reach our data.

```
<DATA> <DATA> <DATA>
                    |
                   rsp
```

We can calculate how many steps to jump back with `length - index - 1`. The length of the stack is stored in the `rbp` register.

Jump back `2` steps:
```
<DATA> <DATA> <DATA>
      |
     rsp
```

We can store references to data inside data by storing multiple indexes. For example, if we want to make a reference to the data at `&stack[3][2][6]` we'd store 3 `u16`s (`3`, `2` and `6`) to represent all the indexes. That means deeply nested structures will have very long references and because they are stored as `u16` you can only have up to `65536` stack items or else the reference will overflow.

# Indexes vs References vs Addresses

I use a lot of words to refer to "thing that points to the stack" and every word means something slightly different. I'll define all the terms I use here:

- Indexes (specifically the `Index` struct) is what the compiler uses at compile-time to uniquely identify values in the stack. You can think of it like a path to some data.
- References are stable pointers to values in the stack. They are still valid even when items before it are resized. They are explained in the [section above](#references). It's basically just an `Index` but stored at runtime instead of compile-time.
- Addresses are the raw unstable memory addresses to values in the stack. They (usually) aren't accessible in the language, which means you can't actually get the raw address of a stack item. It's more of a compiler detail, like when you want to read a memory location you'll need to use the `idx2addr` function that generates the right assembly to convert a compile-time `Index` into an address that can be utilized at runtime.

A stable pointer means it will always point to that object and will only be invalidated if that object is deallocated. An unstable pointer may be invalidated if a stack item is resized, causing the stack to shift around.

# Notes

## Arrays

Array initialization uses `u64`:
```
#     using u64 to create an array
#                 vvv
def []i32 arr = [3u64]i32;
```
Array indexing uses `u16`:
```
#        using u16 to index into an array
#                      vvv
def &i32 arr = (&arr)[0u16];
```

You can't index arrays directly:
```
def i32 arr = arr[0u16];
```
The above code gives the error:
```
array[_] access is only applicable to array references, use `&arr`
```

Instead you have to reference and dereference it:
```
def i32 arr = *(&arr)[0u16];
```

## Most things grow at the start. Structs grow at the end
```
def i32 x = 0;
```
Here is how that is represented on the stack:
```
<SIZE> <DATA>
<4>    <0>
```
And when we resize it...
```
resize(&x, 8);
```
A new size tag gets inserted at the start.
```
<SIZE> <--DATA-->
<8>    <4>    <0>
```
So this program will exit with the exit code `4`:
```
i32 main() {
    def i32 x = 0;
    resize(&x, 8);
    return x;
}
```

Structs on the other-hand...
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
Are stored like so:
```
<SIZE> <------DATA----->
<12>   <3>    <2>    <1>
```
And if we resize...
```
resize(&my_struct, 16);
```
```
<SIZE> <----------DATA-------->
<16>   <12>   <3>    <2>    <1>
```
This program will exit with the exit code `1`:
```
struct MyStruct {
    i32 first,
    i32 second,
    i32 third,
}

i32 main() {
    def MyStruct my_struct;
    my_struct.first = 1;
    my_struct.second = 2;
    my_struct.third = 3;

    resize(&my_struct, 16);

    # assert isnt a real keyword
    # this is just an example
    assert my_struct.first == 1;
    assert my_struct.second == 2;
    assert my_struct.third == 3;

    return my_struct.first;
}
```
