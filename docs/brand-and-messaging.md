# CommonKit brand and messaging

## Positioning

**Category:** Desired-state control plane for agentic development setups.

**Audience:** AI-native developers and small engineering teams using multiple
coding agents across local machines, remote hosts, and projects.

**Problem:** Agent capabilities drift. Developers repeatedly rebuild skills,
instructions, hooks, tools, policies, and credentials for each environment.

**Promise:** Your best development setup, everywhere.

**Enemy:** Fragmented manual setup. CommonKit composes APM, Chezmoi, SkillOpt,
password managers, and MCP infrastructure; it does not position against them.

## Message hierarchy

1. **Start capable.** Carry proven agent capabilities to every target.
2. **Improve continuously.** Produce evidence-backed skill improvements from
   evaluations and approved, redacted usage evidence.
3. **Benefit everywhere.** Approve an improvement once and reconcile it across
   targets.
4. **Stay in control.** Preview, approve, verify, and recover.
5. **Provision secrets safely.** Keep values out of portable configuration and
   resolve references through a password-manager provider.

The first three benefits lead. Governance and safety substantiate the promise.

## Core copy

### Headline

> Your best development setup, everywhere.

### Short description

> CommonKit gives every coding agent your proven skills, tools, and guardrails.
> Improve them from real use. Review what works, then apply it everywhere.

### One-line description

> Version, improve, and safely apply your coding-agent setup across every
> project and machine.

### Improvement loop

> Use → learn → review → improve once → benefit everywhere

### Secret-management line

> Sync the setup. Provision the secrets safely.

Supporting explanation: portable configuration contains references, not secret
values. Password-manager providers provision credentials independently on each
target. Bitwarden Secrets Manager is the first native provider.

## Voice

The voice is a concise, confident craftsperson.

- Use plain claims, concrete verbs, and short sentences.
- Lead with the benefit. Follow with technical evidence.
- Be opinionated without hype.
- Prefer operational language: define, preview, apply, verify, improve.
- Use memorable language sparingly.
- State unfinished capabilities and safety limits directly.

Avoid:

- flowery language;
- cute toolbox metaphors;
- enterprise governance jargon;
- inflated AI claims such as "10x" or "self-improving";
- vague absolute claims about identity, security, or secret protection;
- describing reconciliation as file copying or deployment.

## Visual direction: Terminal Native

The visual system feels operational, direct, and built for expert use.

### Palette

- **Canvas:** near-black green, `#08110C`
- **Surface:** deep green-black, `#0D1911`
- **Primary text:** pale neutral green, `#F1FFF3`
- **Secondary text:** muted green, `#ACD8B7`
- **Status/accent:** clear terminal green, `#77F397`
- **Borders:** dark structural green, `#24412D`

Green communicates healthy system state, not generic sustainability or retro
nostalgia. Use it selectively for status, actions, and proof.

### Typography

- Use a clean sans serif for major headlines.
- Use a modern monospace for navigation, labels, commands, state, diffs, and
  supporting technical copy.
- Keep line lengths short and hierarchy decisive.
- Do not set long marketing paragraphs entirely in monospace.

### Interface language

- Prefer real commands, plans, diffs, receipts, and verification states as
  product proof.
- Square or lightly rounded surfaces are preferred.
- Motion may explain reconciliation or the improvement loop. Avoid decorative
  motion.
- Do not use mascots, glowing AI gradients, glass effects, or stock imagery.
- Avoid excessive terminal syntax in prose. The product is terminal-native,
  not terminal-themed theater.

## Claims discipline

### Approved

- CommonKit carries shared developer capabilities across targets.
- CommonKit previews drift and requires explicit approval before apply.
- CommonKit keeps secret values out of portable configuration and supports
  separately provisioned password-manager references.
- Bitwarden Secrets Manager is the first native password-manager provider.
- Native skill optimization produces isolated, reviewable candidates and
  requires human approval before promotion.

### Qualify

- Skill optimization is under active development until it ships in the current
  release.
- Filesystem apply and successful-run rollback use durable plans, artifacts,
  backups, and authenticated receipts. Installed-platform and snapshot restore
  recovery remain release gates.
- Sensitive-state exclusions cover documented paths and secret-like values;
  they are not a universal identity-isolation guarantee.

### Avoid

- "CommonKit synchronizes secrets."
- "All identity stays local."
- "Skills improve themselves."
- "CommonKit replaces APM, Chezmoi, SkillOpt, or your password manager."
- "One-click rollback" until installed-platform and snapshot recovery gates pass.

## Reader journey

1. Recognize the cost of fragmented agent setups.
2. Understand the promise: one capable setup everywhere.
3. See the capability, improvement, consistency, control, and secret-provisioning
   benefits.
4. Understand define → preview → apply → verify.
5. Verify the safety limits and provider boundaries.
6. Confirm product fit and current availability.
7. See a real multi-target example.
8. Configure one target and run a dry reconciliation.
9. Follow links into architecture, threat model, and provider details.
