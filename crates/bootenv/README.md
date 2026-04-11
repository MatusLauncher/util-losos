# Safe and simple boot environment
`bootenv` is an initramfs environment written entirely in safe Rust. 
# Features
- Safe (in sense that it is written 100% in safe Rust if we don't count dependencies)
- Minimal (in the sense that it requires only one boot parameter, `stage2`)
- Multithreaded (uses async Rust and `spawn` for lightning-fast boots)
- Flexible (Can be used both for booting live OS and also for running OS)
# Building
Just run `scripts/build.sh` with the initramfs tarball/image to use. The script will do everything for you.
# Testing
`cargo test`
# TODO
- [x] Figure out how to pack it.
- [ ] Figure out how to actually boot a tarball from it in QEMU.  
