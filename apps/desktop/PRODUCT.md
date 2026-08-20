# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

Tauri 2 desktop GUI (WebView, no framework). The GUI is an alpha release for
evaluation and feedback. The `commonkit` CLI is the primary product and release
surface. macOS is the alpha GUI's first-class target; the same webview can ship
on Linux and Windows. This is a native-feeling desktop application, not a
website, but its design language is the app's own rather than a reproduction of
AppKit.

## Users

AI-native developers and small engineering teams running multiple coding agents
across local machines, remote hosts, and projects. The primary user in this surface
is a single developer sitting at a Mac on first run, before CommonKit has touched
anything on the machine. They are technical, skeptical of tools that mutate their
environment, and they are deciding in the first minute whether this thing is safe to
point at their home directory.

## Product Purpose

CommonKit makes a hard-won development setup portable: agent instructions, skills,
hooks, tools, and policies materialize consistently on every machine and every coding
agent. Success on the walkthrough surface is a completed connection (GitHub account,
private setup repository, named computer, managed folder) that ends in a **reviewable
plan** rather than an applied change.

## Positioning

Providers compute normalized desired state; CommonKit alone owns policy validation,
plans, target mutation, receipts, verification, and rollback. Nothing is ever applied
without an explicit, per-target confirmation of a specific named plan. The mechanism a
neighboring dotfile manager cannot truthfully copy: every change exists as an
inspectable, content-bound plan with provenance and risk before it exists on disk.

## Operating Context

- First run happens on a developer's own Mac, in a terminal-adjacent workflow, often
  with `gh` already authenticated.
- Credential brokering goes through the installed GitHub CLI; tokens never return to
  the webview.
- The app runs alongside the local background service (`commonkitd`). The
  `commonkit` CLI is the primary complete control surface. The alpha desktop
  reuses the same core and must not redefine CLI behavior or readiness.
- The window is 1200x800, minimum 860x600. There is no mobile viewport.
- The walkthrough is four steps: GitHub, this computer, existing settings, review.

## Capabilities and Constraints

- Vanilla TypeScript + Vite; no UI framework, no component library. All markup is
  template strings; the app re-renders by replacing `innerHTML`.
- Strict CSP: `default-src 'self'`. No remote fonts, scripts, or images. Every asset
  must be bundled or inline.
- Tauri IPC only for privileged work: `@tauri-apps/plugin-dialog` for file pickers,
  updater plugin for signed updates.
- Domain vocabulary that must survive any redesign: Loadout, Target, Adapter, Plan,
  Operation, Provenance, Risk, Receipt, Reconciliation, Drift check, Kit repository,
  Managed root, Principal, Scope, Grant.
- Providers offered at first run: native (empty portable setup), APM, chezmoi.
- Secret values never enter the desktop window. Personal context is encrypted locally
  before synchronization.

## Brand Commitments

- Name: CommonKit. Voice: concise, confident craftsperson. Plain claims, concrete
  verbs, short sentences, technically exact supporting evidence. Opinionated without
  hype.
- Avoid: flowery language, cute metaphors, enterprise jargon, inflated AI claims, and
  any wording that implies CommonKit synchronizes secrets or has already changed the
  machine.
- Binding visual constraint stated by the owner for the walkthrough surface: moody,
  impactful, techy.

## Evidence on Hand

- Real product documentation: `CONTEXT.md`, `docs/adr/` (21 ADRs), `docs/scopes/`.
- Real shipping behavior to show: GitHub device-code sign-in, a real computer ID
  derived from the entered name, real local config/state paths, a real first plan ID.
- No customers, benchmarks, pricing, or testimonials exist. None may be invented.

## Product Principles

1. Nothing is applied without a reviewed, named plan and an explicit confirmation.
2. Show what will happen before it happens; a preview is the product, not a courtesy.
3. Secrets and personal context stay out of portable configuration and out of this window.
4. The CLI is the primary complete control surface. The daemon and alpha
   desktop reuse its core contracts without gating CLI releases.
5. Portability is the promise; every screen should make the machine feel like one of many.

## Accessibility & Inclusion

Keyboard operability and visible focus are required; the app already ships focus-visible
rings, `sr-only` legends, live regions, and `prefers-reduced-motion` handling. WCAG AA
contrast is the floor. No product-specific requirement beyond that has been established.
