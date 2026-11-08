// Preserve whole original ESM modules, discovered by Node's real loader.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {registerHooks} from 'node:module';
import {pathToFileURL,fileURLToPath} from 'node:url';
if (!fs.existsSync('/run/.containerenv') || process.versions.node !== '26.13.0') throw Error('Private admitted build runtime required');
const [effectRoot,output] = process.argv.slice(2);
if (!effectRoot || !output || fs.realpathSync(effectRoot) !== effectRoot) throw Error('Canonical admitted source and fresh output required');
const pkg = JSON.parse(fs.readFileSync(path.join(effectRoot,'package.json'),'utf8'));
if (pkg.name !== 'effect' || pkg.version !== '4.1.2') throw Error('Exact original Effect version required');
const dist = path.join(effectRoot,'dist');
const sources = new Map();
const builtins = new Map();
const hash = value => crypto.createHash('sha256').update(value).digest('hex');
const hooks = registerHooks({
  resolve(specifier,context,nextResolve) {
    const result = nextResolve(specifier,context);
    if (result.url.startsWith("node:")) builtins.set(specifier,result.url);
    return result;
  },
  load(url,context,nextLoad) {
    const loaded = nextLoad(url,context);
    if (url.startsWith('file:')) {
      const file = fileURLToPath(url);
      if (!file.startsWith(dist + path.sep) || loaded.format !== 'module') throw Error('Unexpected original dependency outside admitted Effect graph');
      const bytes = fs.readFileSync(file);
      const source = bytes.toString('utf8');
      if (!Buffer.from(source).equals(bytes)) throw Error('Original module is not exact UTF-8');
      const name = 'effect/' + path.relative(dist,file).split(path.sep).join('/');
      sources.set(name,{path:name,sha256:hash(bytes),source});
    } else if (!url.startsWith('node:')) throw Error('Unexpected original module scheme');
    return loaded;
  },
});
try {
  const DateTime = await import(pathToFileURL(path.join(dist,'DateTime.js')).href);
  const Option = await import(pathToFileURL(path.join(dist,'Option.js')).href);
  const Effect = await import(pathToFileURL(path.join(dist,'Effect.js')).href);
  // Discover original synchronous clock initialization, without replacing it.
  const now = Effect.runSync(DateTime.now);
  if (Option.isNone(DateTime.make(DateTime.formatIso(now)))) throw Error('Original clock initialization failed');
} finally { hooks.deregister(); }
const packet = {schema:1,effectVersion:pkg.version,
  entries:{DateTime:'effect/DateTime.js',Option:'effect/Option.js',Effect:'effect/Effect.js'},
  modules:[...sources.values()].sort((a,b)=>a.path<b.path?-1:a.path>b.path?1:0),
  builtins:Object.fromEntries([...builtins].sort(([a],[b])=>a<b?-1:a>b?1:0)),
  license:fs.readFileSync(path.join(effectRoot,'LICENSE'),'utf8')};
if (!packet.modules.length || packet.modules.length > 1024) throw Error('Bounded complete original module graph required');
const bytes = Buffer.from(JSON.stringify(packet));
if (bytes.length > 8*1024*1024) throw Error('Original module packet exceeds source bound');
fs.writeFileSync(output,bytes,{flag:'wx',mode:0o600});
console.log(JSON.stringify({modules:packet.modules.length,bytes:bytes.length,sha256:hash(bytes),originalModuleBytes:packet.modules.reduce((n,m)=>n+Buffer.byteLength(m.source),0)}));
