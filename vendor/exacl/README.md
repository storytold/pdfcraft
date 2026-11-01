# Exacl

[![CRATE]][crates] [![API]][docs] [![CI]][actions] [![COV]][codecov]

[CRATE]: https://img.shields.io/crates/v/exacl
[crates]: https://crates.io/crates/exacl
[CI]: https://github.com/byllyfish/exacl/workflows/CI/badge.svg
[actions]: https://github.com/byllyfish/exacl/actions?query=branch%3Amain
[API]: https://docs.rs/exacl/badge.svg
[docs]: https://byllyfish.github.io/exacl
[COV]: https://codecov.io/gh/byllyfish/exacl/branch/main/graph/badge.svg?token=SWkSyVc1w6
[codecov]: https://codecov.io/gh/byllyfish/exacl

Rust library to manipulate file system access control lists (ACL) on `macOS`, `Linux`, and `FreeBSD`.

## Example

```rust
use exacl::{getfacl, setfacl, AclEntry, Perm};

// Get the ACL from "./tmp/foo".
let mut acl = getfacl("./tmp/foo", None)?;

// Print the contents of the ACL.
for entry in &acl {
    println!("{entry}");
}

// Add an ACL entry to the end.
acl.push(AclEntry::allow_user("some_user", Perm::READ, None));

// Set the ACL for "./tmp/foo".
setfacl(&["./tmp/foo"], &acl, None)?;
```

## Benefits

- Supports the Posix ACL's used by Linux and FreeBSD.
- Supports the extended ACL's used by macOS and FreeBSD/NFSv4.
- Supports reading/writing of ACL's as delimited text.
- Supports serde (optional) for easy reading/writing of ACL's to JSON, YAML and other common formats.

There are two main flavors of ACL's: Posix and Extended.

| Operating System | Posix |  Extended |
| -----------------| -------- | -------- |
| Linux | ✅ | ❌ |
| macOS | ❌ | ✅ |
| FreeBSD | ✅ | ✅ |

## API

This module provides two high level functions, `getfacl` and `setfacl`.

- `getfacl` retrieves the ACL for a file or directory.
- `setfacl` sets the ACL for files or directories.

On Linux and FreeBSD, the ACL contains entries for the default ACL, if
present.

Both `getfacl` and `setfacl` work with a `Vec<AclEntry>`. The
`AclEntry` structure contains five fields:

- kind : `AclEntryKind` - the kind of entry (User, Group, Other, Mask,
    or Unknown).
- name : `String` - name of the principal being given access. You can
    use a user/group name, decimal uid/gid, or UUID (on macOS).
- perms : `Perm` - permission bits for the entry.
- flags : `Flag` - flags indicating whether an entry is inherited, etc.
- allow : `bool` - true if entry is allowed; false means deny. Linux only
    supports allow=true.


## More Examples

Here are some more examples showing how to use the library.

Get an ACL in common delimited string format:

```rust
    let acl = exacl::getfacl("/tmp/file", None)?;
    let result = exacl::to_string(&acl)?;
```

Get an ACL in JSON format:

```rust
    let acl = exacl::getfacl("/tmp/file", None)?;
    let result = serde_json::to_string(&acl)?;
```

Create a linux ACL for permissions that allow the owning user and group to read/write a file 
but no one else except for "fred".

```rust
    let mut acl = exacl::from_mode(0o660);
    acl.push(AclEntry::allow_user("fred", Perm::READ | Perm::WRITE, None));
    exacl::setfacl(&["/tmp/file"], &acl, None)?;
```

Create a linux ACL for directory permissions that gives full access to the owning user and group
and read-only access to members of the accounting group. Any sub-directories created should 
automatically have the same ACL (via the default ACL).

```rust
    let mut acl = exacl::from_mode(0o770);
    acl.push(AclEntry::allow_group(
        "accounting",
        Perm::READ | Perm::EXECUTE,
        None,
    ));

    // Make default_acl a copy of access_acl with the DEFAULT flag set.
    let mut default_acl: Vec<AclEntry> = acl.clone();
    for entry in &mut default_acl {
        entry.flags |= Flag::DEFAULT;
    }
    acl.append(&mut default_acl);
    
    exacl::setfacl(&["./tmp/dir"], &acl, None)?;
```

## Build and Test

On Linux, you must install the `libacl1-dev` package to build exacl. The integration tests 
require `shunit2` which can be installed via apt or homebrew. 

```
sudo apt install libacl1-dev shunit2
```

To run the unit tests with debug logging, type: `RUST_LOG=debug cargo test`

To run the integration tests, type:

```
cargo test --features serde; ./tests/run_tests.sh
```

### Bindgen Feature

If there is a problem building exacl on your system, try enabling the bindgen feature.

```
cargo test --features bindgen
```
