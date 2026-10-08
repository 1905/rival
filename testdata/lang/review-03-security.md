# Path traversal in the archive extractor

An attacker who controls the archive can include an entry named `../../etc/cron.d/x`, and the extractor will happily write it outside of the target directory.

The code in `extract.rs:118` joins the entry name with the destination without validating it; therefore any relative component like `..` escapes the sandbox.

Failure scenario: a malicious release archive is downloaded by `rival update`, an entry with `../` is extracted, and a file is overwritten in the user's home directory.

Suggestion: Reject every entry whose normalized path does not start with the destination directory. Log the rejected name.
