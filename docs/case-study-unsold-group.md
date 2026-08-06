# The kit Unsold.Group carries to ship 100x

Unsold.Group is a small, AI-native company. Its leverage does not come from one
model or one prompt. It comes from carrying proven working context from one
agent, machine, and project to the next.

“Ship 100x” is the direction, not a synthetic benchmark. The practical goal is
to remove repeated setup and let every improvement compound across the places
where the team works.

## The problem: capability was trapped in place

The working setup grew through daily use: coding standards, research methods,
specialist agents, deployment scripts, safety rules, service definitions, and
project-specific context. It worked, but its parts lived in different native
directories on a Mac, in repositories, and on an always-on Linux host.

That created three forms of drag:

1. A different agent could start without the skills available to the previous
   agent.
2. A remote machine could have a different tool or service version from the
   local machine.
3. A colleague could receive team instructions only by inheriting somebody
   else's personal setup.

The valuable thing was not the files. It was the accumulated judgment inside
them, plus the rules for where that judgment should apply.

## The carried kit

The August 2026 dogfood inventory counted the working surface before adoption:

| Portable capability | Observed Unsold.Group kit |
| --- | ---: |
| Active skills | 52 |
| On-demand skill library | 130 |
| Reusable coding agents | 14 |
| Specialist development personas | 7 |
| Repository bundles | 7 |
| Local utility scripts | 25 |
| Primary targets | macOS arm64 workstation + Linux x64 VPS |

The kit also declares shell and Git configuration, project loadouts, user
services, MCP services, and references to credentials. Secret values do not
enter the portable configuration.

These counts are a point-in-time inventory, not product limits. Their value is
that CommonKit turns a large, lived-in setup into explicit desired state rather
than a checklist somebody must remember.

## One source, several destinations

CommonKit separates what should travel from what should remain specific:

| Layer | What Unsold.Group puts there |
| --- | --- |
| Organization policy | The security floor every target must preserve |
| Personal kit | Skills, agents, scripts, preferences, and credential references |
| Project loadout | Repository-specific capabilities and context |
| Target overrides | Services, ports, paths, and machine-specific facts |

Providers compute the desired resources. CommonKit then inspects the target,
shows a plan, applies approved operations through adapters, verifies the
result, and records a receipt.

This boundary matters for colleagues. The shared organization repository can
carry common policy, documentation, and capabilities. Each colleague keeps a
private personal kit. Joining the team does not require copying another
person's identity or private context.

It also matters across agents. CommonKit owns the normalized intent while an
adapter renders the native configuration each supported agent expects. The
source is portable without pretending every agent has the same configuration
format.

## Where the leverage compounds

The “100x” effect is cumulative:

- Improve a skill once, then make the reviewed version available to every
  selected agent and target.
- Put project context beside the repository so a fresh clone starts with the
  same operating knowledge.
- Give a remote execution target the declared loadout instead of rebuilding it
  from a remembered sequence of commands.
- Detect version and configuration drift before it becomes a mysterious agent
  failure.
- Add a colleague through shared organization context without exposing private
  personal context.

The result is not an agent that magically works 100 times faster. It is a team
that stops paying the setup tax on every agent, machine, project, and handoff.

## What the dogfood run exposed

Portability is more credible when its boundary is visible. The inventory found
that CommonKit can manage the high-value agent corpus, project bundles, files,
supported user services, remote files, the persistent MCP relay, and credential
references today.

It also found gaps: package and language-toolchain installation, adoption of
some pre-existing symlink layouts, system-level services, and parts of the
macOS external-provider path remain outside the current managed surface.
CommonKit reports those boundaries instead of silently claiming a complete
clone of an arbitrary machine.

The source inventory and current limitations are recorded in
[the dogfood inventory](plans/dogfood-inventory.md). Platform support requires
native validation evidence; macOS results never stand in for Linux or Windows.

## What we learned

1. **Context is the durable asset.** Agent-specific files are delivery formats.
2. **Personal and shared context need different ownership.** Composition must
   not erase confidentiality.
3. **Machine differences belong in the model.** Portability is not identical
   bytes everywhere.
4. **A preview is part of the product.** Moving context must not mean granting
   silent mutation.
5. **Honest gaps build trust.** Unmanaged state should appear as a boundary,
   not disappear behind a success message.

## Try the same path

Install the CLI, create or connect a kit, and generate a read-only first plan:

```sh
npm install --global @alunsoldgroup/commonkit
commonkit init create --help
commonkit status
commonkit sync
```

Initialization requires explicit publication consent. Synchronization creates
a plan; it does not apply that plan. Review the diff before you approve target
mutation.

[Return to the README](../README.md) ·
[Read the architecture](../CONTEXT.md) ·
[Review the threat model](THREAT-MODEL.md)
