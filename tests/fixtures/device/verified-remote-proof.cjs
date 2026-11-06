/* Synthetic-only proof. No SSH, SDK, daemon, installer, shell, or host process is started. */
'use strict';
const fs = require('node:fs'); const path = require('node:path'); const os = require('node:os'); const crypto = require('node:crypto'); const assert = require('node:assert/strict'); const { EventEmitter } = require('node:events');
if (!fs.existsSync('/run/.containerenv') && !(process.env.CI === 'true' && process.env.GITHUB_ACTIONS === 'true' && fs.existsSync('/.dockerenv'))) throw Error('Private test container required.');
const { runVerifiedRemote } = require(process.argv[2]);
const owner = 'abcdef012345abcdef012345';
function fixture() {
 const home = fs.mkdtempSync(path.join(os.tmpdir(), 'o3-verified-remote-'));
 const root = path.join(home, '.t3/device/tools/expo-device-hub@0.19.1'); const packageRoot = path.join(root, 'node_modules/expo-device-hub');
 fs.mkdirSync(path.join(packageRoot, 'dist/server'), { recursive: true });
 fs.writeFileSync(path.join(packageRoot, 'package.json'), JSON.stringify({ name: 'expo-device-hub', version: '0.19.1' }));
 fs.writeFileSync(path.join(packageRoot, 'dist/server/cli.mjs'), '// Synthetic entry; never executed\n');
 const files = {};
 for (const name of ['node_modules/expo-device-hub/package.json', 'node_modules/expo-device-hub/dist/server/cli.mjs']) files[name] = crypto.createHash('sha256').update(fs.readFileSync(path.join(root, name))).digest('hex');
 fs.writeFileSync(path.join(root, '.o3-device-install.json'), JSON.stringify({ format: 1, package: 'expo-device-hub', version: '0.19.1', files }));
 const record = path.join(home, '.t3/device/hosts', owner, 'hub.o3-verified.json');
 let clock = 0; let running = null; let launches = 0; let healthy = true; let correctOwnership = true; const signals = []; const forbidden = [];
 const filesystem = new Proxy(fs, { get(target, key) { if (['rmSync', 'rmdirSync', 'unlink', 'rm', 'rmdir'].includes(key)) return () => { forbidden.push(key); throw Error('Tree deletion forbidden by test fixture'); }; return target[key]; } });
 const host = { platform: 'linux', versions: { node: '26.13.0' }, execPath: '/fixture/node', pid: 900, env: { PRIVATE_FIXTURE: '1' }, kill(pid, signal) { if (signal === 0) { if (running?.pid === pid && running.alive) return; throw Object.assign(Error('gone'), { code: 'ESRCH' }); } signals.push({ pid, signal }); if (running && pid === -running.pid) running.alive = false; else throw Error('Attempted to signal a non-owned fixture process'); } };
 const system = { fs: filesystem, path, os: { homedir: () => home }, crypto, process: host, now: () => clock, sleep: async milliseconds => { clock += milliseconds; }, fetch: async () => ({ ok: healthy }), reservePort: async () => 43210,
  spawnSync(command, args) { if (command === 'adb') return { status: 0 }; if (command === 'ps') return { status: 0, stdout: correctOwnership && running ? `${host.execPath} ${running.args.join(' ')} O3_DEVICE_OWNER=${owner} O3_DEVICE_NONCE=${running.env.O3_DEVICE_NONCE}` : 'unrelated fixture process' }; throw Error('Unexpected synthetic command: ' + command); },
  spawn(command, args, options) { assert.equal(command, host.execPath); assert.equal(options.shell, false); assert.equal(options.detached, true); launches++; const child = Object.assign(new EventEmitter(), { pid: 901, exitCode: null, signalCode: null, unref() {}, kill(signal) { host.kill(-this.pid, signal); } }); running = { pid: child.pid, args, env: options.env, alive: true }; queueMicrotask(() => child.emit('spawn')); return child; },
 };
 const call = (mode, receipt) => runVerifiedRemote({ protocol: 1, owner, mode, ...(receipt ? { receipt } : {}) }, system);
 return { home, root, packageRoot, record, system, call, signals, forbidden, launches: () => launches, alive: () => !!running?.alive, unhealthy: () => { healthy = false; }, mismatch: () => { correctOwnership = false; } };
}
(async () => {
 const checks = [];
 {
  const f = fixture(); const probe = await f.call('probe'); assert.equal(probe.verifiedInstalled, true); assert.equal(probe.platforms[1].available, true); assert.equal(f.launches(), 0); assert.equal(fs.existsSync(path.dirname(f.record)), false); checks.push('read-only verified probe');
  const first = await f.call('start'); assert.match(first.receipt, /^[a-f0-9]{48}$/); assert.equal(f.alive(), true); assert.equal(f.launches(), 1);
  const second = await f.call('start'); assert.equal(second.receipt, first.receipt); assert.equal(f.launches(), 1); checks.push('owned healthy reuse');
  await assert.rejects(f.call('stop', '0'.repeat(48)), /receipt changed/); assert.equal(f.alive(), true); assert.equal(f.signals.length, 0); checks.push('stale receipt cannot stop replacement');
  const stopped = await f.call('stop', first.receipt); assert.equal(stopped.stopped, true); assert.equal(f.alive(), false); assert.equal(fs.existsSync(f.record), false); assert.equal(fs.readdirSync(path.dirname(f.record)).filter(name => name.includes('.retained.')).length, 1); assert.equal(f.forbidden.length, 0); checks.push('owned stop retains record without tree deletion');
 }
 {
  const f = fixture(); fs.writeFileSync(path.join(f.packageRoot, 'dist/server/cli.mjs'), '// Changed private fixture\n'); const probe = await f.call('probe'); assert.equal(probe.verifiedInstalled, false); await assert.rejects(f.call('start'), /inventory changed/); assert.equal(f.launches(), 0); checks.push('changed installation is refused');
 }
 {
  const f = fixture(); const started = await f.call('start'); fs.writeFileSync(path.join(f.packageRoot, 'dist/server/cli.mjs'), '// Changed after launch\n'); await f.call('stop', started.receipt); assert.equal(f.alive(), false); checks.push('owned stop survives a missing or changed installation');
 }
 {
  const f = fixture(); const started = await f.call('start'); f.mismatch(); await assert.rejects(f.call('stop', started.receipt), /ownership could not be verified/); assert.equal(f.signals.length, 0); assert.equal(f.alive(), true); assert.equal(fs.existsSync(f.record), true); checks.push('unverifiable process remains untouched');
 }
 {
  const f = fixture(); f.unhealthy(); await assert.rejects(f.call('start'), /did not become ready/); assert.equal(f.alive(), false); assert.equal(f.signals[0].signal, 'SIGTERM'); assert.equal(f.forbidden.length, 0); checks.push('failed readiness closes owned process scope');
 }
 {
  const f = fixture(); const target = path.join(f.home, 'private-target'); fs.mkdirSync(target); fs.writeFileSync(path.join(target, 'kept'), 'retained'); fs.symlinkSync(target, path.join(f.root, 'fixture-link')); await assert.rejects(f.call('start'), /symbolic link/); assert.equal(fs.readFileSync(path.join(target, 'kept'), 'utf8'), 'retained'); assert.equal(f.launches(), 0); checks.push('links point only within private temporary fixture and are refused');
 }
 {
  const f = fixture(); fs.mkdirSync(path.dirname(f.record), { recursive: true }); fs.writeFileSync(f.record, JSON.stringify({ owner, pid: 123 })); await assert.rejects(f.call('start'), /unrecognized/); assert.equal(f.signals.length, 0); assert.equal(fs.readFileSync(f.record, 'utf8'), JSON.stringify({ owner, pid: 123 })); checks.push('foreign or corrupt process records remain immutable');
 }
 {
  const f = fixture(); fs.mkdirSync(path.dirname(f.record), { recursive: true }); const lock = path.join(path.dirname(f.record), 'runtime.o3-verified.lock'); fs.writeFileSync(lock, 'unknown owner'); await assert.rejects(f.call('start'), /EEXIST/); assert.equal(fs.readFileSync(lock, 'utf8'), 'unknown owner'); checks.push('unknown lock is not cleared');
 }
 {
  const f = fixture(); await assert.rejects(f.call('stop', '0'.repeat(48)), /stop cannot be confirmed/); assert.equal(f.signals.length, 0); checks.push('missing process record cannot report successful stop');
 }
 console.log(JSON.stringify({ checks, count: checks.length }));
})().catch(error => { console.error(error); process.exitCode = 1; });
