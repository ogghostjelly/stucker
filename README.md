# Footguns

## You can't take a reference to an array element.
```
def []u8 arr = [1]u8;
arr[0] = 1;

# this gets a reference to a copy of the array data
# not the array element itself 
def &u64 x = &arr[0]
```

## Numbers grow at the start. Structs grow at the end.
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