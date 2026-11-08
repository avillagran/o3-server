// A sealed in-memory source graph. It neither rewrites nor approximates dates.
import {registerHooks} from 'node:module';
import {createHash} from 'node:crypto';
const ROOT = 'o3-owned:/';
const hash = value => createHash('sha256').update(value).digest('hex');
export async function createNativeVisitCore(packetText,expectedDigest) {
  if (!/^[0-9a-f]{64}$/.test(expectedDigest) || hash(packetText) !== expectedDigest) throw Error('Native source packet admission failed');
  const packet = JSON.parse(packetText);
  if (packet.schema !== 1 || packet.effectVersion !== '4.1.2' || !Array.isArray(packet.modules) || !packet.modules.length || packet.modules.length > 1024) throw Error('Native source packet shape refused');
  const sources = new Map();
  for (const item of packet.modules) {
    if (typeof item.path !== 'string' || !/^effect\/[A-Za-z0-9_./-]+\.js$/.test(item.path) || item.path.split('/').some(p=>!p||p==='.'||p==='..') || typeof item.source !== 'string' || hash(item.source) !== item.sha256 || sources.has(ROOT+item.path)) throw Error('Original module identity refused');
    sources.set(ROOT+item.path,item.source);
  }
  const hooks = registerHooks({
    resolve(specifier,context,nextResolve) {
      if (context.parentURL?.startsWith(ROOT) && Object.hasOwn(packet.builtins,specifier)) return nextResolve(packet.builtins[specifier],context);
      if (specifier.startsWith(ROOT) || context.parentURL?.startsWith(ROOT)) {
        const url = new URL(specifier,context.parentURL || ROOT).href;
        if (!sources.has(url)) throw Error('Unadmitted native module resolution');
        return {url,shortCircuit:true};
      }
      return nextResolve(specifier,context);
    },
    load(url,context,nextLoad) {
      if (url.startsWith(ROOT)) {
        if (!sources.has(url)) throw Error('Unadmitted native module source');
        return {format:'module',source:sources.get(url),shortCircuit:true};
      }
      return nextLoad(url,context);
    },
  });
  try {
    const DateTime = await import(ROOT+packet.entries.DateTime);
    const Option = await import(ROOT+packet.entries.Option);
    const Effect = await import(ROOT+packet.entries.Effect);
    const normalize = value => {
      const result = DateTime.make(value);
      return Option.isNone(result) ? null : {date:result.value,epoch:DateTime.toEpochMillis(result.value),canonical:DateTime.formatIso(result.value)};
    };
    return {
      // Callers supply only schema-admitted strings or nullable stored watermarks.
      normalize(value) {
        const result = normalize(value);
        return result === null ? null : {epoch:result.epoch,canonical:result.canonical};
      },
      prepare(input,recordThreadOpen) {
        // Projection/deleted checks happen in Rust before entering this capability.
        if (input.previousVisitedAt !== null && typeof input.previousVisitedAt !== 'string') return {kind:'invalidPrevious',clockReads:0,historyCalls:0};
        const previous = input.previousVisitedAt === null ? null : normalize(input.previousVisitedAt);
        if (input.previousVisitedAt !== null && previous === null) return {kind:'invalidPrevious',clockReads:0,historyCalls:0};
        const visited = normalize(input.visitedAt);
        if (visited === null) return {kind:'invalidTimestamp',clockReads:0,historyCalls:0};
        const movesForward = previous === null || DateTime.toEpochMillis(visited.date) > DateTime.toEpochMillis(previous.date);
        const occurredAt = Effect.runSync(DateTime.now);
        const at = DateTime.toEpochMillis(occurredAt);
        const opens = input.opened ? recordThreadOpen(input.previousOpens,at,input.searchQuery) : null;
        return {kind:'prepared',visitedAt:visited.canonical,visitedEpoch:visited.epoch,movesForward,previousCanonical:previous?.canonical??null,
          at,occurredAt:DateTime.formatIso(occurredAt),opens,clockReads:1,historyCalls:input.opened?1:0};
      },
      close() {hooks.deregister();},
    };
  } catch (error) {hooks.deregister();throw error;}
}
