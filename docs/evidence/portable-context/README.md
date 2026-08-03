# Portable-context native validation evidence

Portable-context v1 support is declared per operating system. A platform is
supported only when this directory contains a passing, schema-valid evidence
record produced by the installed lifecycle driver on that native platform.

Required evidence targets:

- `macos.json`
- `linux.json`
- `windows.json`

Generate each record on its named native platform with
`scripts/qualify-eight-flows.sh <evidence-root> <installed-bin>`. Review the
resulting `portable-context-evidence.json` and copy it to the corresponding
target above. Evidence is never synthesized from another operating system.

Evidence records contain only versions, opaque digests, named validation steps,
outcomes, timestamps, and artifact hashes. They must never contain tokens,
keys, decrypted profile values, personal ciphertext, repository credentials,
or arbitrary environment dumps.

The release gate fails closed when evidence is missing, belongs to another
revision, reports a skipped/failed step, or cannot be validated. macOS evidence
cannot stand in for Linux or Windows, and remote JavaScript results do not
qualify a native provider or installed lifecycle.
