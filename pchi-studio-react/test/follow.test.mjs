// Tests for which runtime Studio talks to. Run: npm run test:sdk
import test from 'node:test'
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'

const sdk = createRequire(import.meta.url)('../.sdk-test/sdk.cjs')
const { chooseRuntime, normalizeBase } = sdk

const A = (epoch = 0) => ({ role: 'active', epoch })
const S = { role: 'standby' }
const F = { role: 'fenced', epoch: 1 }

test('with no backup address Studio always talks to the primary', () => {
  assert.equal(chooseRuntime('primary', null, A(1), false), 'primary')
  assert.equal(chooseRuntime('backup', null, A(1), false), 'primary')
})

test('Studio stays where it is while that runtime is the active one', () => {
  assert.equal(chooseRuntime('primary', A(), S, true), 'primary')
  assert.equal(chooseRuntime('backup', S, A(1), true), 'backup')
})

test('it moves to the backup when the primary is gone, stopped or not the active one', () => {
  assert.equal(chooseRuntime('primary', null, A(1), true), 'backup')
  assert.equal(chooseRuntime('primary', F, A(1), true), 'backup')
  assert.equal(chooseRuntime('primary', S, A(1), true), 'backup')
})

test('it does not move to a backup that is only a standby, or to one that does not answer', () => {
  assert.equal(chooseRuntime('primary', null, S, true), 'primary')
  assert.equal(chooseRuntime('primary', null, null, true), 'primary')
  assert.equal(chooseRuntime('backup', null, null, true), 'backup', 'it stays rather than guessing')
})

test('it comes back to the primary only when the primary is active and the backup is not', () => {
  assert.equal(chooseRuntime('backup', A(), S, true), 'primary')
  assert.equal(chooseRuntime('backup', A(), F, true), 'primary')
})

test('when both claim to be active the later takeover wins, and a tie stays put', () => {
  assert.equal(chooseRuntime('primary', A(0), A(1), true), 'backup')
  assert.equal(chooseRuntime('backup', A(0), A(1), true), 'backup')
  assert.equal(chooseRuntime('backup', A(2), A(1), true), 'primary')
  assert.equal(chooseRuntime('primary', A(1), A(1), true), 'primary')
  assert.equal(chooseRuntime('backup', A(1), A(1), true), 'backup')
})

test('a backup address is made into a base URL or refused in words', () => {
  assert.deepEqual(normalizeBase('10.0.0.6:3100'), { base: 'http://10.0.0.6:3100' })
  assert.deepEqual(normalizeBase(' https://show-b.example:8443/ '), { base: 'https://show-b.example:8443' })
  assert.deepEqual(normalizeBase(''), { base: '' })
  assert.ok(normalizeBase('http://x/api/role').error)
  assert.ok(normalizeBase('not an address').error)
})
