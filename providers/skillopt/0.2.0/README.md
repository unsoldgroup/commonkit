# SkillOpt 0.2.0 provider lock

This directory describes the external SkillOpt provider supported by the
`skillopt-sleep-v1` adapter. It does not contain SkillOpt source or Python
dependencies.

- PyPI wheel: `skillopt-0.2.0-py3-none-any.whl`
- Wheel SHA-256: `818db802507c6f82553fd24c75aa70c953ab0a712647f60e68e4595052c4b150`
- Git tag commit: `e4ea6a6771e797ef820cdd8bfea64c57e0481065`
- License: MIT

An installed provider environment is compatible only after CommonKit verifies
this identity and passes the synthetic fixtures under `fixtures/`.

Operations and security guidance:

- [`docs/runbooks/skillopt-provider.md`](../../../docs/runbooks/skillopt-provider.md)
- [`docs/security/skillopt-provider-threat-model.md`](../../../docs/security/skillopt-provider-threat-model.md)

Only the `skillopt-sleep run` compatibility path is supported. The `adopt`,
`schedule`, `unschedule`, and `--auto-adopt` paths are prohibited.

