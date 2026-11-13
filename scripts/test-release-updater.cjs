#!/usr/bin/env node
// Optional integration regression: install release-please@17.3.0 outside the repo.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {createRequire} = require('node:module');
const {execFileSync} = require('node:child_process');

const toolRoot = path.resolve(process.argv[2] || '');
assert(process.argv[2], 'usage: node scripts/test-release-updater.cjs <external npm prefix>');
const rpRoot = path.join(toolRoot, 'node_modules/release-please');
assert.equal(JSON.parse(fs.readFileSync(path.join(rpRoot, 'package.json'))).version, '17.3.0');
const rpRequire = createRequire(path.join(rpRoot, 'package.json'));
const TOML = rpRequire('@iarna/toml');
const {GenericToml} = require(path.join(rpRoot, 'build/src/updaters/generic-toml'));
const {CargoToml} = require(path.join(rpRoot, 'build/src/updaters/rust/cargo-toml'));
const {CargoLock} = require(path.join(rpRoot, 'build/src/updaters/rust/cargo-lock'));
const {ReleasePleaseManifest} = require(path.join(rpRoot, 'build/src/updaters/release-please-manifest'));
const {Version} = require(path.join(rpRoot, 'build/src/version'));
const root = path.resolve(__dirname, '..');
assert(toolRoot !== root && !toolRoot.startsWith(root + path.sep), 'npm prefix must be outside the checkout');
const fixture = fs.mkdtempSync(path.join(toolRoot, 'updater-fixture-'));
const names = ['Cargo.toml', 'Cargo.lock', 'fuzz/Cargo.lock', '.release-please-manifest.json'];
const originals = Object.fromEntries(names.map(name => [name, fs.readFileSync(path.join(root, name), 'utf8')]));
for (const name of names) {
  fs.mkdirSync(path.dirname(path.join(fixture, name)), {recursive: true});
  fs.writeFileSync(path.join(fixture, name), originals[name]);
}
const before = TOML.parse(originals['fuzz/Cargo.lock']);
const old = before.package.find(pkg => pkg.name === 'remik').version;
const next = old.replace(/-rc\.(\d+)$/, (_, number) => `-rc.${Number(number) + 1}`);
assert.notEqual(next, old, 'this regression needs an rc version');
const version = Version.parse(next);
const warnings = [];
const logger = {info() {}, error(message) {throw new Error(message);}, warn(message) {warnings.push(message);}};
const legacy = new GenericToml("$.package[?(@.name == 'remik')].version", version);
assert.equal(legacy.updateContent(originals['fuzz/Cargo.lock'], logger), originals['fuzz/Cargo.lock']);
assert(warnings.some(message => message.startsWith('No entries modified')), 'legacy selector must reproduce the bug');
warnings.length = 0;
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json')));
const extra = config.packages['.']['extra-files'].find(file => file.path === 'fuzz/Cargo.lock');
const updated = new GenericToml(extra.jsonpath, version).updateContent(originals['fuzz/Cargo.lock'], logger);
const after = TOML.parse(updated);
const expected = structuredClone(before);
expected.package.find(pkg => pkg.name === 'remik').version = next;
assert.deepEqual(after, expected, 'only remik.version may change in the fuzz lock');
assert.equal(Version.parse(after.package.find(pkg => pkg.name === 'remik').version).toString(), next);
assert.equal(warnings.length, 0, 'correct selector must not silently miss');
fs.writeFileSync(path.join(fixture, 'fuzz/Cargo.lock'), updated);
const versionsMap = new Map([['remik', version]]);
fs.writeFileSync(path.join(fixture, 'Cargo.toml'), new CargoToml({version, versionsMap}).updateContent(originals['Cargo.toml'], logger));
fs.writeFileSync(path.join(fixture, 'Cargo.lock'), new CargoLock(versionsMap).updateContent(originals['Cargo.lock'], logger));
fs.writeFileSync(path.join(fixture, '.release-please-manifest.json'), new ReleasePleaseManifest({version, versionsMap: new Map([['.', version]])}).updateContent(originals['.release-please-manifest.json']));
const cargo = TOML.parse(fs.readFileSync(path.join(fixture, 'Cargo.toml'), 'utf8'));
const lock = TOML.parse(fs.readFileSync(path.join(fixture, 'Cargo.lock'), 'utf8'));
const manifest = JSON.parse(fs.readFileSync(path.join(fixture, '.release-please-manifest.json')));
assert.equal(cargo.package.version, next);
assert.equal(lock.package.find(pkg => pkg.name === 'remik' && !pkg.source).version, next);
assert.deepEqual(manifest, {'.': next});
fs.writeFileSync(path.join(fixture, 'release-please-config.json'), JSON.stringify(config));
execFileSync('python3', ['-c',
  "import runpy, sys; from pathlib import Path; runpy.run_path(sys.argv[1])['check'](Path(sys.argv[2]), sys.argv[3], exact_only=True); print('Python tomllib + release contract: PASS')",
  path.join(root, 'scripts/check-release.py'), fixture, `v${next}`], {stdio: 'inherit'});
for (const name of names) assert.equal(fs.readFileSync(path.join(root, name), 'utf8'), originals[name], `checkout changed: ${name}`);
console.log(`release-please 17.3.0: ${old} -> ${next}; only remik changed; root/fuzz versions and manifest agree`);
console.log(`Parseable copy fixture: ${fixture}`);
