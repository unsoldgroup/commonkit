# CommonKit styleguide

This experimental first-party APM package exports the `technical-writing`
skill for Claude and Codex. CommonKit loadouts must select it explicitly. The
skill is routed and does not add always-on instructions.

The evaluation scorer is internal evidence. It does not certify Simplified
Technical English, judge factual quality, or define a public lint gate.

## Experimental platform support

- macOS arm64
- Linux x64

Windows is not supported by this experimental version. Add Windows only after
the same native scorer, pinned APM install, frozen replay, audit, and
Claude/Codex output-integrity checks have passed and their evidence is
committed. This package-specific limit does not change CommonKit's wider
platform roadmap.
