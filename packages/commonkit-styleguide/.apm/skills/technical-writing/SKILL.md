---
name: technical-writing
description: Write or revise technical prose with stable terms, plain words, direct verbs, active voice, short sentences, focused paragraphs, and clear procedures. Route for READMEs, documentation, procedures, runbooks, safety text, error messages, PR descriptions, release notes, deprecation notices, and code comments. Do not route for source code, identifiers, command syntax, quoted source material, marketing copy, essays, or creative writing.
---

# Technical writing

Apply this guide only to the requested prose. Preserve all facts, required
details, constraints, examples, and warnings. This guide improves writing
form. It cannot make weak or false content correct.

## Select a mode

Use `strict` for procedures, runbooks, safety text, and error messages.

Use `technical` for READMEs, documentation, PR descriptions, release notes,
deprecation notices, and code comments.

Do not apply either mode to source code, identifiers, command syntax, quoted
source material, marketing copy, essays, creative writing, or prose whose
distinct voice is required.

## Core rules

- Use one term for one thing. Do not rename an item within the same text.
- Prefer a short common word when it keeps the exact meaning.
- Use active voice when the actor is known and relevant.
- Use a direct verb for an action. Avoid nominalizations such as "perform an
  analysis."
- Remove stacked auxiliaries, empty lead-ins, and unsupported claims.
- Keep each sentence focused on one idea.
- Keep each paragraph focused on one topic.
- Put a condition before the action that depends on it.
- Use a numbered vertical list for an ordered procedure.
- Put one action in each procedure step. Start the step with an imperative
  verb.

## Strict mode

- Apply every core rule.
- Limit an instruction to 20 words.
- Limit a descriptive sentence to 25 words.
- Do not use contractions or semicolons.
- Use articles where they make the noun clear.
- Replace an `-ing` main verb with a simple tense when the meaning stays exact.

## Technical mode

- Apply every core rule.
- Prefer short sentences, but keep necessary technical qualifications.
- Use the established domain term even when it is uncommon.
- Preserve useful variation when repetition would make long prose mechanical.
- Do not trade accuracy, required detail, or voice-sensitive source text for a
  lower heuristic score.

## Check the result

1. Confirm that every source fact and required detail remains.
2. Confirm that the selected mode matches the prose scope.
3. Confirm that excluded content is unchanged.
4. Check terminology, verbs, sentence length, paragraphs, and procedures.
5. Return only the requested text unless the user asks for an explanation.

See `PROVENANCE.md` for the upstream source, license, adaptation notes, and
section-level retention map.
