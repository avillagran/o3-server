/* Verified-installed remote hub lifecycle. No downloads, archive extraction or tree removal. */
'use strict';
async function runVerifiedRemote(request, system) {
  const { fs, path, os, crypto, process: host, spawn, spawnSync, sleep, fetch, reservePort } = system;
  const now = system.now ?? Date.now;
  const fail = (reason) => { throw new Error(reason); };
  if (request.protocol !== 1 || !/^[a-f0-9]{24}$/.test(request.owner ?? '') || !['probe', 'start', 'stop'].includes(request.mode)) fail('Invalid remote device lifecycle request.');
  if (request.mode === 'stop' && !/^[a-f0-9]{48}$/.test(request.receipt ?? '')) fail('Stopping a remote device hub requires its owner receipt.');
  if (host.platform === 'win32') fail('SSH device hosts currently require macOS or Linux.');
  if (Number(host.versions.node.split('.')[0]) < 22) fail('Node 22 or newer is required on the device host.');
  const home = os.homedir();
  const root = path.join(home, '.t3', 'device');
  const state = path.join(root, 'hosts', request.owner);
  const tool = path.join(root, 'tools', 'expo-device-hub@0.19.1');
  const recordPath = path.join(state, 'hub.o3-verified.json');
  const lockPath = path.join(state, 'runtime.o3-verified.lock');
  const directory = (value, create = false) => {
    const resolved = path.resolve(value); let current = path.parse(resolved).root;
    for (const part of resolved.slice(current.length).split(path.sep).filter(Boolean)) {
      current = path.join(current, part);
      let metadata;
      try { metadata = fs.lstatSync(current); }
      catch (error) { if (error.code !== 'ENOENT' || !create) throw error; fs.mkdirSync(current, { mode: 0o700 }); metadata = fs.lstatSync(current); }
      if (!metadata.isDirectory() || metadata.isSymbolicLink()) fail('Remote device directory contains a link or non-directory.');
    }
    return resolved;
  };
  const read = (file, limit) => {
    const initial = fs.lstatSync(file);
    if (!initial.isFile() || initial.isSymbolicLink() || initial.size > limit) fail('Remote device file is not regular or exceeds its size limit.');
    const descriptor = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    try {
      const metadata = fs.fstatSync(descriptor);
      if (!metadata.isFile() || metadata.size > limit) fail('Remote device file is not regular or exceeds its size limit.');
      const result = Buffer.alloc(metadata.size + 1); let length = 0;
      while (length < result.length) { const count = fs.readSync(descriptor, result, length, result.length - length, null); if (!count) break; length += count; }
      if (length !== metadata.size) fail('Remote device file changed while being read.');
      return result.subarray(0, length);
    } finally { fs.closeSync(descriptor); }
  };
  const hashFile = (file, expectedSize) => {
    const descriptor = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    try {
      const metadata = fs.fstatSync(descriptor); if (!metadata.isFile() || metadata.size !== expectedSize) fail('Remote device inventory changed type or size.');
      const digest = crypto.createHash('sha256'); const buffer = Buffer.alloc(65536); let length = 0;
      while (true) { const count = fs.readSync(descriptor, buffer, 0, Math.min(buffer.length, expectedSize - length + 1), null); if (!count) break; length += count; if (length > expectedSize) fail('Remote device inventory file grew.'); digest.update(buffer.subarray(0, count)); }
      if (length !== expectedSize) fail('Remote device inventory file shrank.'); return digest.digest('hex');
    } finally { fs.closeSync(descriptor); }
  };
  const readJson = (file, limit = 64 * 1024) => JSON.parse(read(file, limit).toString('utf8'));
  const maybeJson = (file) => { try { return readJson(file); } catch (error) { if (error.code === 'ENOENT') return null; throw error; } };
  const verify = () => {
    directory(tool);
    const manifest = readJson(path.join(tool, '.o3-device-install.json'), 32 * 1024 * 1024);
    if (manifest.format !== 1 || manifest.package !== 'expo-device-hub' || manifest.version !== '0.19.1' || !manifest.files || typeof manifest.files !== 'object' || Array.isArray(manifest.files)) fail('Remote device tool manifest pin mismatch.');
    const actual = new Map(); const pending = [tool]; let size = 0;
    while (pending.length) {
      const parent = pending.pop();
      for (const name of fs.readdirSync(parent)) {
        const file = path.join(parent, name); const metadata = fs.lstatSync(file);
        if (metadata.isSymbolicLink()) fail('Remote device tool contains a symbolic link.');
        if (metadata.isDirectory()) { pending.push(file); continue; }
        if (!metadata.isFile()) fail('Remote device tool contains a special file.');
        const relative = path.relative(tool, file).split(path.sep).join('/');
        if (relative === '.o3-device-install.json') continue;
        size += metadata.size;
        if (size > 4 * 1024 * 1024 * 1024 || actual.size >= 100000) fail('Remote device tool inventory exceeds its size limit.');
        actual.set(relative, hashFile(file, metadata.size));
      }
    }
    const entries = Object.entries(manifest.files);
    if (entries.length !== actual.size || entries.some(([name, hash]) => actual.get(name) !== hash)) fail('Remote device tool inventory changed.');
    const packagePath = path.join(tool, 'node_modules', 'expo-device-hub');
    const metadata = readJson(path.join(packagePath, 'package.json'), 1024 * 1024);
    if (metadata.name !== 'expo-device-hub' || metadata.version !== '0.19.1') fail('Remote device package pin mismatch.');
    const entry = path.join(packagePath, 'dist', 'server', 'cli.mjs');
    if (!fs.lstatSync(entry).isFile()) fail('Remote device entry is not a regular file.');
    return entry;
  };
  const run = (command, args) => spawnSync(command, args, { encoding: 'utf8', timeout: 5000, maxBuffer: 1024 * 1024, shell: false });
  const ios = host.platform === 'darwin' && run('xcrun', ['simctl', 'help']).status === 0;
  const android = run('adb', ['version']).status === 0;
  const platforms = [{ platform: 'ios', available: ios, ...(!ios ? { reason: 'iOS needs macOS with Xcode and working xcrun simctl.' } : {}) }, { platform: 'android', available: android, ...(!android ? { reason: 'Android SDK missing. Set ANDROID_HOME or put adb on the SSH PATH.' } : {}) }];
  let entry = path.join(tool, 'node_modules', 'expo-device-hub', 'dist', 'server', 'cli.mjs'); let verifiedInstalled = true; let detail;
  try { entry = verify(); } catch (error) { if (request.mode === 'start') throw error; verifiedInstalled = false; detail = error.message; }
  const summary = { protocol: 1, owner: request.owner, nodePath: host.execPath, platforms, verifiedInstalled, ...(detail ? { detail } : {}) };
  if (request.mode === 'probe') return summary;
  if (request.mode === 'start' && !ios && !android) fail(platforms.map(platform => platform.reason).join(' '));
  directory(state, true);
  const nonce = crypto.randomBytes(24).toString('hex');
  const lock = { protocol: 1, owner: request.owner, nonce, pid: host.pid };
  const lockFile = fs.openSync(lockPath, 'wx', 0o600);
  try { fs.writeFileSync(lockFile, JSON.stringify(lock)); fs.fsyncSync(lockFile); } finally { fs.closeSync(lockFile); }
  const alive = (pid) => { try { host.kill(pid, 0); return true; } catch (error) { if (error.code === 'ESRCH') return false; throw error; } };
  const validRecord = (record) => record && record.protocol === 1 && record.owner === request.owner && /^[a-f0-9]{48}$/.test(record.nonce ?? '') && Number.isSafeInteger(record.pid) && record.pid > 0 && Number.isSafeInteger(record.port) && record.port > 0 && record.port < 65536 && record.entryPath === entry;
  const owns = (record) => {
    if (!validRecord(record) || !alive(record.pid)) return false;
    // Inspect only in memory; never return process environments or command lines to callers.
    const command = run('ps', ['eww', '-p', String(record.pid), '-o', 'command=']);
    if (command.status !== 0) return false;
    const output = command.stdout ?? '';
    return output.includes(entry) && output.includes(`--port ${record.port}`) && new RegExp(`(?:^|\\s)O3_DEVICE_OWNER=${request.owner}(?:\\s|$)`).test(output) && new RegExp(`(?:^|\\s)O3_DEVICE_NONCE=${record.nonce}(?:\\s|$)`).test(output);
  };
  const healthy = async (port) => { try { return (await fetch(`http://127.0.0.1:${port}/readyz`, { signal: AbortSignal.timeout(2000), redirect: 'error' })).ok; } catch { return false; } };
  const stop = async (record) => {
    if (!validRecord(record)) fail('Refusing to stop an unrecognized remote device process record.');
    if (!alive(record.pid)) return;
    if (!owns(record)) fail('Remote device process ownership could not be verified.');
    host.kill(-record.pid, 'SIGTERM');
    const deadline = now() + 5000;
    while (alive(record.pid) && now() < deadline) await sleep(50);
    if (alive(record.pid)) { if (!owns(record)) fail('Remote device process ownership changed during stop.'); host.kill(-record.pid, 'SIGKILL'); }
  };
  const quarantineRecord = () => { fs.renameSync(recordPath, `${recordPath}.retained.${nonce}`); };
  let spawned = null; let spawnedRecord = null;
  try {
    let record = maybeJson(recordPath);
    if (record && !validRecord(record)) fail('Refusing to replace an unrecognized remote device process record.');
    if (request.mode === 'stop') {
      if (!record) fail('Owned remote process record is missing; stop cannot be confirmed.');
      if (record && record.nonce !== request.receipt) fail('Remote device receipt changed; retained process was not stopped.');
      if (record) { await stop(record); quarantineRecord(); }
      return { ...summary, stopped: true };
    }
    if (record && alive(record.pid)) {
      if (!owns(record)) fail('Remote device process ownership could not be verified.');
      if (await healthy(record.port)) return { ...summary, hubPort: record.port, receipt: record.nonce, helpers: { serveSimAxSettings: null, serveSimCli: null } };
      await stop(record);
    }
    if (record) quarantineRecord();
    const port = await reservePort();
    const args = [entry, '--port', String(port), '--host', '127.0.0.1', '--hide-sidebar', '--hide-boot-device', ...(host.platform === 'darwin' ? [] : ['--platform', 'android'])];
    spawned = spawn(host.execPath, args, { cwd: state, detached: true, shell: false, stdio: 'ignore', env: { ...host.env, O3_DEVICE_OWNER: request.owner, O3_DEVICE_NONCE: nonce, FORCE_COLOR: '0', NO_COLOR: '1' } });
    await new Promise((resolve, reject) => { spawned.once('spawn', resolve); spawned.once('error', reject); });
    record = { protocol: 1, owner: request.owner, nonce, pid: spawned.pid, port, entryPath: entry };
    spawnedRecord = record;
    const temporary = `${recordPath}.${nonce}`; const descriptor = fs.openSync(temporary, 'wx', 0o600);
    try { fs.writeFileSync(descriptor, JSON.stringify(record)); fs.fsyncSync(descriptor); } finally { fs.closeSync(descriptor); }
    fs.renameSync(temporary, recordPath);
    const deadline = now() + 30000;
    while (spawned.exitCode === null && spawned.signalCode === null && now() < deadline) {
      if (await healthy(port)) { spawned.unref(); spawned = null; return { ...summary, hubPort: port, receipt: nonce, helpers: { serveSimAxSettings: null, serveSimCli: null } }; }
      await sleep(200);
    }
    fail('Verified remote device hub did not become ready.');
  } finally {
    if (spawned?.pid) { if (spawnedRecord) await stop(spawnedRecord); else if (spawned.exitCode === null && spawned.signalCode === null) spawned.kill('SIGTERM'); }
    const current = maybeJson(lockPath);
    if (current?.owner === lock.owner && current?.nonce === lock.nonce && current?.pid === lock.pid) fs.unlinkSync(lockPath);
  }
}
if (typeof module !== 'undefined') module.exports = { runVerifiedRemote };
if (typeof o3VerifiedRemoteRequest !== 'undefined') {
  const net = require('node:net');
  runVerifiedRemote(o3VerifiedRemoteRequest, {
    fs: require('node:fs'), path: require('node:path'), os: require('node:os'), crypto: require('node:crypto'), process,
    spawn: require('node:child_process').spawn, spawnSync: require('node:child_process').spawnSync,
    sleep: milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds)), fetch,
    reservePort: () => new Promise((resolve, reject) => { const listener = net.createServer(); listener.once('error', reject); listener.listen(0, '127.0.0.1', () => { const port = listener.address().port; listener.close(() => resolve(port)); }); }),
  }).then(value => console.log(JSON.stringify(value)), error => { console.error(JSON.stringify({ error: 'VerifiedRemoteDeviceError', message: error.message })); process.exitCode = 1; });
}
