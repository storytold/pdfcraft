# PdfCraft temporary exacl patch

Source: [byllyfish/exacl](https://github.com/byllyfish/exacl/tree/18275aa1a82aae9a786b108b0db0992a4d06e6e2), exact upstream commit `18275aa1a82aae9a786b108b0db0992a4d06e6e2` (2026-10-27).
The upstream MIT `LICENSE.txt` is unchanged.
This copy retains the package manifest, build script, source, checked-in bindings and upstream README and security policy.
CI scripts, examples, tests and changelog are omitted; no binary assets are included.
Upstream source is otherwise unchanged except for the marked `PdfCraft patch:` in `src/lib.rs`.

`copyfacl_fd` exposes a safe macOS descriptor-to-descriptor native extended ACL copy through upstream's existing native `Acl::read_fd` and `Acl::write_fd` methods.
A fallible caller validator inspects a native-ID entry view of the captured ACL before it can be applied.
The same native ACL object is then passed directly to the operating system without reconstructing entries, resolving principal names or discarding ACL-level flags.
It copies neither contents, POSIX modes nor extended attributes, and propagates read and write failures.
PdfCraft's platform staging regressions cover explicit ACL preservation, ordered entries, inherited staging refusal, deny-delete refusal before payload and failure cleanup.

Propose the small facade upstream and remove this vendor when an upstream release provides an equivalent opaque native ACL copy.
The already merged descriptor entry API alone is insufficient: entry reconstruction omits ACL-level flags.
No new unsafe code is introduced by this patch.
