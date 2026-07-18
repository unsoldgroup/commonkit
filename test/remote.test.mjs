import assert from 'node:assert/strict'
import test from 'node:test'

import { shellQuote } from '../src/remote.mjs'

test('shellQuote preserves multi-word commands and embedded quotes as one argument', () => {
  const value = `node -p "process.versions.node"; printf '%s\\n' "hello world"`
  const quoted = shellQuote(value)

  assert.equal(quoted.startsWith("'"), true)
  assert.equal(quoted.endsWith("'"), true)
  assert.match(quoted, /'"'"'/)
})
