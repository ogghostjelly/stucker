# Print help text
help:
    just -l

# Build the code in the `examples/` directory and place the binary in `/tmp/stucker-main`
build-example:
    cargo run -- examples/main.skr -o examples/main.asm
    nasm -gdwarf -felf64 examples/main.asm -o /tmp/stucker-main.o
    objdump -d /tmp/stucker-main.o > examples/main.objdump
    ld /tmp/stucker-main.o -o /tmp/stucker-main
