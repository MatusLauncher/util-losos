## [unreleased]

### Bug Fixes

- Add Qwen to .gitignore
- Add missing gpuman submodule and update submodule pointers
- Justfile backtick exit-code crash on missing kernel/initramfs
- Update user and isoman submodules to fix default login credentials
- Set shared CARGO_TARGET_DIR in pages job for excluded submodule docs
- Exclude submodule crates from workspace to fix CI pages stage
- Make launch.sh more robust and configurable
- Make poll interval configurable to fix flaky CI test
- Resolve port conflict in parallel client tests
- Format code and add libclang-dev to CI
- Refactor GitLab CI pipeline and simplify rustfmt config
- Add tokio and ureq dependencies
- Quote GitLab CI commands to prevent variable expansion
- Auto-commit formatting changes in CI
- Replace docker with nerdctl in compose execution
- Format workspace members list in Cargo.toml
- Ignore code snippets in cargo test.
- Replace zig cc/c++ wrappers with native gcc/g++
- Add cluman library export and MODE-aware container builds
- Merge building and copying to one layer
- Use syscalls instead.
- Add fallbacks if given filesystem was not found
- Build initramfs under fakeroot for /dev/console creation
- Extract only required nerdctl binaries to prevent CI disk exhaustion
- Restore console output and init script execution in initramfs
- Build locally from the CI
- *(actman)* Comment out unneeded stuff
- *(launch)* Disable podman layer cache on build
- Correct three bugs preventing initramfs from booting

### Documentation

- Add auto commit and push workflow to CLAUDE.md
- Add LICENSE and README files to all submodules
- Update book submodule with submodule architecture docs
- Remove unnecessary `unsafe` and `static mut` from CONT_F
- Replace podman with nerdctl
- Add Rustdoc for actman, updman, and testman
- Update

### Features

- Mount data drive and virtual FS before switch_root
- Add bootenv crate and Secure Boot / encrypted boot support
- Add container cache mounts and CI cargo cache
- Hook isoman config to updman via file-based configuration
- Add git-cliff configuration and pre-commit hook for automated changelog generation
- Update agent documentation
- Merge cluman and sshman.
- Replace launch.sh with Justfile
- Add LUKS/LVM/NFS support with auto-detection
- Add gpuman submodule and integration benches
- Add GitLab CI pipelines to all submodule crates
- Update book, sshman, and user submodules
- Document sshman in CLAUDE.md and README
- Add sshman submodule
- Remove sshman crate from the monorepo
- Refactor CI docs build and add sshman submodule
- Split monorepo into submodules under mtos-v2 namespace
- Add overlayfs_fuse for persistent state management
- Dynamically locate initramfs instead of hardcoding filename
- Remove unused import and simplify mode assignment
- Add actman dependency to perman and userman crates
- Replace rustyx with expressjs
- Reorganize userman and perman crates to top-level user directory
- Add docs submodule and simplify MUSL build process
- Migrate crates/user workspace into root Cargo workspace
- Add Zig-based cross-compilation toolchain and isoman support
- Update .gitignore to match all ISO variants
- Make ISO output path mode-aware when omitted
- Remove Containerfile and add isoman + mkbootimg to bench crate
- Add mkbootimg dependency for bootimage creation
- Replace mkbootimg CLI with library wrapper
- Add --gsi CLI flag to isoman
- Add GSI builder (fastboot + Odin) to isoman
- Add GSI constants to isoman lib
- Convert benchmark suite to libtest smoke tests
- Change RebootCMD::from to accept &str instead of &String
- Add Mode import and skip controller mode in set_mode
- Remove unused scopeguard test code
- Consolidate imports and remove unused dependencies
- Add GitLab Pages deployment for API documentation
- Add crate-level documentation and improve inline doc comments
- Add reboot and poweroff endpoints to cluman server
- Simplify CI pipeline to basic linting and testing
- Document pakman module with comprehensive README and doc comments
- Add pakman crate and consolidate workspace dependencies
- Add licenses.
- Remove unused functions.
- Generize the codebase.
- Update the AI policy.
- Add comprehensive integration tests and documentation for util-mdl
- Add nextest config, rustfmt settings, and CI improvements
- Add benchmark suite for all crates
- Add ipnet dependency and refactor cluman into modules
- Add clap CLI parsing to controller mode
- Add cluman crate
- Reorganize tests into lib.rs for better integration testing
- Add os.iso to .gitignore
- Add unit tests and refactor testable code into library functions
- Add `isoman` crate and integrate with testman and launch.sh
- Add dhcman DHCP client and networking support
- Add data drive mounting and network config scripts
- Fix the init system for good.
- Add GitLab CI pipeline and testman documentation
- Add testman workspace crate and launch --test mode
- *(updman)* Finished the updating mechanism
- *(updman)* Close to completing the update mechanism
- Provide local support for CLI flags by actman.
- Work on kernel paramater autodetection.
- Work on actman and add preboot section.
- Work on the initramfs OS builder
- Initial
- Initial

### Miscellaneous

- Update isoman, updman submodules; clean up Justfile
- Update bootenv, isoman, docs submodules — UKI disk image, data_drive= only
- Update actman, bootenv, docs submodules — move mounting to bootenv
- Update Justfile to reflect isoman CLI simplification
- Migrate docs pipeline to Docusaurus/Bun; update submodules
- Update dhcman, isoman, testman submodules — auto-detect network interfaces
- Update submodules — gpuman (NVLink/topology/MIG/CDI/per-GPU), cluman (GPU scheduling), docs (GPU scaling)
- Update isoman submodule — embed SB key as LUKS keyfile
- Update isoman submodule with symlink fix
- Update cluman and isoman submodules with simplify fixes
- Update submodules with bug fixes and hardening
- Update submodules and increase default CPUS to 4
- Update isoman submodule with serial console fix
- Update isoman submodule with lbman feature flag
- Update isoman submodule with sh init script fix
- Update dhcman submodule with lbman load balancer
- Update submodules to point to losos-linux
- Update all submodules to latest
- Update all submodules with hooks migration
- Update cluman submodule with fast_rsync implementation
- Update cluman submodule with content synchronisation
- Update cluman submodule to include round-robin storage balancing
- Remove unused dependencies across multiple crates
- Update cluman and isoman submodule pointers
- Remove all sshman references (merged into cluman)
- Format.

### Refactor

- Strip bootenv to data_drive= only, remove stage2/luks/switch_root
- Refactor CmdLineOptions::param_search to accept &str
- *(ci)* Refactor CI/CD pipeline into discrete stages
- *(ci)* Build ISO with isoman, add comprehensive tests
- Refactor cluman controller to one-shot task pusher
- Extract library code from isoman, updman, and testman
- Replace mount subprocess with rustix syscall and fix Containerfile symlinks

### Testing

- Add pakman benchmarks and make install/run public

