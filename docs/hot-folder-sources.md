# Hot Folder source adapters

Hot Folder can watch a local/network folder directly, or explicitly stage files
from FTP/FTPS, Outlook, or a PDFCraft project into a task's local inbox. No source
is connected, authenticated or downloaded merely by importing a task configuration.
These adapters are native Rust; the browser/WASM build does not include them.

## Configuration and credentials

`SourceConfig` is a tagged JSON object. Its default is `{"kind":"local"}`. Remote
configurations contain a credential **identifier**, never passwords, device secrets,
access tokens or refresh tokens. Configuration export is not a credential backup.
Source actions default to `{"kind":"keep"}`. `move` and `delete` require an explicit
task setting and a durable successful output receipt for the exact source SHA-256.

Services derive `CredentialStore::for_config(config_path, root)` from their selected
configuration. Its location is `<configuration parent>/secrets`. It does not fall
back to a GUI user's OS vault. Provision credentials as the account that runs the
service, or as an elevated Windows administrator where the installer authorizes it:

| Platform | Required credential storage |
| --- | --- |
| Windows service | `%ProgramData%/PdfCraft/HotFolder/secrets`; protected DACL with only SYSTEM, Administrators and LocalService; owner one of those identities |
| Linux service | Service-account-owned `secrets` directory, mode `0700`; files `0600`, one hard link; provision as the configured service user |
| macOS LaunchDaemon | Same owner/mode requirements, under the selected service user's configuration directory; provision as that user |

The Windows installer must create the secrets directory **before** provisioning,
without inheriting the ordinary Users read access of the parent. Required DACL:
`D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;S-1-5-19)`.
The code checks both owner and grants before reading or writing secrets. Directory
symlinks, junctions and linked files are refused. Unix provisioning never changes
an existing directory's ownership or permissions silently.

An elevated Unix GUI can provision a **non-root** service account by opening its
existing configuration through `CredentialStore::for_config`. This creates an
explicit administrator context bound to that configuration's UID/GID. The regular
configuration must already be `0600`, its parent `0700`, and both must have the
same service owner/group. Each operation rechecks that binding; a missing,
re-owned, linked or broadly readable configuration is rejected. New secret files,
temporary generations and the lock are assigned to the bound UID/GID **before**
secret contents are written, and remain `0600`. An absent secrets directory can
be created as `0700` for exactly that owner. Existing files are never silently
re-owned. This context is for provisioning, not for running the OCR worker as root.

Service secrets are permission-protected files, not application-level encrypted
files. OS access control is their security boundary. Per-user OS vaults are also
available through an explicit `CredentialStore::OsVault` execution context for
portable, interactive workers; they are unsuitable for an unrelated boot service.
The credential-store context is not serialized into task configuration.

Secret changes use generation-tagged chunks and an atomic verified pointer, with
a cross-process file lock in service mode. A failed refresh leaves the previous
complete generation available. Secret buffers are zeroized when dropped where
the API owns them; service files must be included only in protected system backups.
Error messages never include passwords, tokens or raw Graph response bodies.

## FTP and explicit FTPS

```json
{
  "kind": "ftp",
  "host": "scanner.example.org",
  "port": 21,
  "security": "explicit_tls",
  "username": "scans",
  "credential_id": "office-scanner",
  "folder": "/incoming",
  "recursive": false,
  "action": {"kind": "move", "destination": "/processed"}
}
```

Save the password separately with `save_ftp_password(store, id, password)`.
`explicit_tls` is the default. It requires `AUTH TLS`, validates the certificate
and hostname against OS trust roots, and protects data connections with `PROT P`.
There is no certificate-ignore option and no fallback to clear text. Plain FTP is
available only through explicit `security: "plain"`; it exposes credentials and
contents to the network. SFTP and implicit FTPS on port 990 are not implemented.

The server must provide UTF-8 RFC 3659 `MLSD` listings with modification times.
Binary passive transfers use EPSV, with a PASV fallback, always to the control
connection's peer address. Server-provided alternate data addresses are ignored.
Active FTP, symlink entries and directory traversal are refused. Recursion is
limited to 16 levels and enumeration to 10,000 entries. Archive directories must
already exist outside the watched directory.

Each transfer checks metadata before and after downloading. Before moving or
deleting an original the adapter checks its version and re-downloads its hash.
Archive names have a stable source-ID prefix; an occupied, different target is
never intentionally replaced. FTP has no portable atomic compare-and-delete or
no-clobber rename primitive: a server-side concurrent writer can still race the
final command. Use an inbox whose completed uploads are not modified by other
writers; keeping originals avoids this protocol limitation.

## Outlook via Microsoft Graph

Register your own Microsoft Entra public-client application and enable the device
authorization flow. Choose supported account types suitable for your account;
there is no borrowed or bundled application/client ID. Organization consent or
conditional-access policy may prevent device login.

```json
{
  "kind": "outlook",
  "client_id": "YOUR-OWN-APPLICATION-GUID",
  "tenant": "common",
  "credential_id": "office-mail",
  "folder_id": "inbox",
  "action": {"kind": "keep"}
}
```

An explicit sign-in returns Microsoft's verification URL, user code and expiry.
The device secret is held only in the selected protected credential store.
Polling respects the server interval, `slow_down`, denial and expiry. Refresh
tokens support unattended work after the user authorizes the application. Refresh
checks the account ID and refuses a change of mailbox. Revocation, expired consent
or organizational restrictions require a new explicit sign-in.

Scopes are `offline_access User.Read Mail.Read` when keeping originals and
`offline_access User.Read Mail.ReadWrite` when moving/deleting. Changing between
these permission sets requires another sign-in. Mail folders can be listed and
selected by ID, including child folders. Only Microsoft's global Graph cloud is
supported; national-cloud Graph endpoints and on-premises Exchange are not.

The adapter downloads non-inline PDF, PNG, JPEG and TIFF **file attachments**.
Cloud/reference attachments, attached messages and inline signatures are skipped.
Immutable message IDs survive moves within the same mailbox. Every source version
records mailbox identity, immutable message ID, attachment ID, modification time
and size. Pagination and redirects cannot send bearer credentials to another host.

Original actions affect the **whole email**, never individual attachments. All
supported attachments in the email must have confirmed output receipts. The
adapter fetches the complete current attachment list and verifies every hash
before moving the email. Local PDF/image filters that exclude an attachment can
therefore leave the message in its original folder; they never authorize losing
an unprocessed attachment. An email with more than 128 supported attachments or
more than 512 MiB of supported attachments is rejected intact.

`delete` moves the email to **Deleted Items**, using Graph's move API; it never
permanently deletes the message. Move/delete retries first reconcile the immutable
message's current folder. An uncertain POST is not blindly retried. If a message
or attachment changes after processing, the adapter retains it and reports the
conflict. Concurrent external edits between the final check and move are not
transactionally locked by Graph; use a dedicated intake folder.

## Open project manifests

A PDFCraft project is a UTF-8 JSON manifest, not a proprietary ABBYY OCR project.
The manifest keeps the requested input order, including after a restart:

```json
{
  "format": "pdfcraft-hot-folder-project",
  "version": 1,
  "files": ["chapter-2/scan.pdf", "chapter-1/scan.png"]
}
```

Select it with `{"kind":"project","manifest":"/absolute/project.json"}`.
Paths in `files` are unique relative paths beneath the manifest directory, with
no `..`, absolute paths or symlink/junction components. All paths honor `--root`.
Missing, never-staged sources are errors, so a grouped export cannot silently
omit them. Manifest entries remain after confirmed move/delete actions. Projects
support the same PDF/PNG/JPEG/TIFF inputs; Office files and ABBYY project formats
are not accepted.

## Worker integration, restart and limits

Call `validate` and `local_paths` during cross-task validation. The latter returns
project originals for overlap checks against other task inputs, archives and
outputs. Call blocking `stage` and `acknowledge` only on a worker thread.

`stage` writes only into the selected inbox, using temporary files, SHA-256 checks
and atomic no-clobber publication. It returns both new and previously staged inputs.
`relative` is the logical source path used for grouping; `staged_relative` is the
flat filename used for reading. Configuration changes cannot reuse another
configuration's journal: choose a fresh staging inbox.

The core must durably save **all** configured outputs and journal their source hash
before calling `acknowledge`. Acknowledgement records readiness before any remote
action. Failed remote actions remain in `pending_acknowledgements`; the core must
recheck its own durable output receipt before retrying them. Ready sources are not
downloaded again. A missing local stage file after readiness is allowed. The
adapter never removes staging files itself.

Limits: 512 MiB per file and per stage batch, 128 files per batch, 10,000 listed
entries/journal versions, 8 MiB metadata responses, 200 Graph pagination requests,
five-minute operation deadline. `has_more` signals a truncated batch; Folder/All
grouping must wait/report the limit rather than publish a partial group. Per-file
tasks can advance through successive batches. A journal reaching its version limit
requires a fresh task/inbox; journal data is never silently discarded.

Cancellation is checked between source operations and transfer blocks. FTP socket
operations time out after ten seconds; Graph HTTP operations after 45 seconds.
TLS internal reads also enforce the overall deadline, including slowly sending
peers. Cancellation does not undo an already confirmed server action; the durable
journal reconciles it on restart. Partially downloaded files are not published.
On Unix the source journal and project archive directory entries are synced after
atomic publication, before proceeding to an original-file action. A directory
sync failure stops the operation; its journal can be reconciled on retry.

## Validation evidence and boundaries

`cargo test -p pdfcraft-engine hot_folder_sources --lib` exercises synthetic local
project files, durable receipts, cancellation, changed originals, no-clobber
archives, queue truncation, missing entries and manifest order. Loopback socket
tests execute real passive FTP listing/download/move and TLS trust/hostname checks.
Graph transport fixtures exercise device polling, refresh/account binding,
pagination confinement, whole-email completeness, content verification and retry
reconciliation. Credential tests cover generation integrity, permission refusal
and the Windows protected-ACL policy. Test certificates are generated at runtime;
no third-party scan, certificate, account or mailbox is distributed as a fixture.

These tests do not constitute a live Microsoft tenant, production FTP deployment
or cross-platform installer acceptance test. No contributor account is connected
by the test suite. Service startup, account consent and target-platform credential
provisioning should be checked in the installer acceptance environment.

## Protocol references

- [Linux fsync and directory-entry persistence](https://man7.org/linux/man-pages/man2/fsync.2.html)
- [RFC 2428: EPSV](https://www.rfc-editor.org/rfc/rfc2428.html)
- [RFC 3659: MLSD facts](https://www.rfc-editor.org/info/rfc3659/)
- [RFC 4217: FTP over TLS](https://www.rfc-editor.org/info/rfc4217/)
- [Microsoft device authorization grant](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code)
- [Outlook immutable identifiers](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)
- [List message attachments](https://learn.microsoft.com/en-us/graph/api/message-list-attachments?view=graph-rest-1.0)
- [Read attachment content](https://learn.microsoft.com/en-us/graph/api/attachment-get?view=graph-rest-1.0)
- [List mail folders](https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders?view=graph-rest-1.0)
- [List messages in a folder](https://learn.microsoft.com/en-us/graph/api/mailfolder-list-messages?view=graph-rest-1.0)
- [Move a message](https://learn.microsoft.com/en-us/graph/api/message-move?view=graph-rest-1.0)
