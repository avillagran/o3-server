#!/usr/bin/env node
// Execute only pinned pure home resolvers; Effect services are explicit deterministic fixtures.
import {readFileSync,writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
import * as NodePath from 'node:path';
const [directory,output]=process.argv.slice(2);
if(!directory||!output)throw Error('Usage: generate-provider-home-fixtures.mjs source-directory output.json');
const read=name=>readFileSync(directory+'/'+name,'utf8');
const codex=read('CodexHomeLayout.ts');const claude=read('ClaudeHome.ts');const driver=read('CodexDriver.ts');const expansion=read('pathExpansion.ts');
const codexPure=codex.slice(codex.indexOf('function resolveHomePath('),codex.indexOf('const CodexShadowHomeContext ='));
const claudePure=claude.slice(claude.indexOf('export const resolveClaudeHomePath ='),claude.indexOf('// A Claude Code session'));
const driverPure=driver.slice(driver.indexOf('    const environmentHome = processEnv.CODEX_HOME;'),driver.indexOf('    const continuationIdentity = codexContinuationIdentity(homeLayout);'));
if(!codexPure.startsWith('function resolveHomePath(')||!claudePure.includes('export const makeClaudeEnvironment =')||!driverPure.includes('yield* resolveCodexHomeLayout'))throw Error('Unrecognized pinned home resolver layout');
const prelude=`const NodePath=globalThis.__homePaths;const service=value=>({*[Symbol.iterator](){return value;}});const Path={Path:service({...NodePath,resolve:(...parts)=>NodePath.resolve('/fixture-cwd',...parts)})};const HostProcess={HomeDirectory:service('/fixture-home')};const Effect={fn:()=>fn=>fn};`;
globalThis.__homePaths=NodePath.posix;
const source=prelude+expansion.slice(expansion.indexOf('export function expandHomePath('))+codexPure+claudePure+`export function* codexDriverHome(config,processEnv){${driverPure}return homeLayout;}`;
const api=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const run=effect=>{const step=effect.next();if(!step.done)throw Error('Pure resolver requested an unexpected external effect');return step.value;};
const definitions=[
 ['codex',{},{}],
 ['codex',{homePath:'~/.codex-work'},{}],
 ['codex',{homePath:'profiles/../codex'},{}],
 ['codex',{homePath:'/srv/shared',shadowHomePath:'accounts/a'},{}],
 ['codex',{shadowHomePath:'~/account-a'},{CODEX_HOME:'/ignored'}],
 ['codex',{}, {CODEX_HOME:'/env/codex'}],
 ['codex',{}, {CODEX_HOME:'~/inherited-codex'}],
 ['codex',{homePath:'~'},{}],
 ['codex',{homePath:' /explicit/with-space '},{}],
 ['codex',{homePath:'~//nested'},{}],
 ['codex',{homePath:'~\\/nested'},{}],
 ['codex',{}, {CODEX_HOME:' /env/with-space '}],
 ['codex',{homePath:' ',shadowHomePath:' '},{}],
 ['codex',{homePath:'/explicit',shadowHomePath:''},{CODEX_HOME:'/ignored'},[{name:'CODEX_HOME',value:'/instance'}]],
 ['claudeAgent',{},{}],
 ['claudeAgent',{homePath:'~/.claude-work'},{CLAUDE_CONFIG_DIR:'/ignored',KEEP:'value'}],
 ['claudeAgent',{homePath:'profiles/../claude',shadowHomePath:'/ignored'},{}],
 ['claudeAgent',{}, {CLAUDE_CONFIG_DIR:'/env/claude'}],
 ['claudeAgent',{}, {CLAUDE_CONFIG_DIR:'~/literal'}],
 ['claudeAgent',{}, {CLAUDE_CONFIG_DIR:' /env/literal ',KEEP:'value'}],
 ['claudeAgent',{homePath:'~\\work'},{}]
];
const cases=definitions.map(([kind,provided,inherited,environment=[]])=>{
 const config={homePath:'',shadowHomePath:'',...provided};const processEnv={...inherited,...Object.fromEntries(environment.map(e=>[e.name,e.value]))};
 const expected=kind==='codex'?run(api.codexDriverHome(config,processEnv)):{sharedHomePath:run(api.resolveClaudeHomePath(config,processEnv)),environment:run(api.makeClaudeEnvironment(config,processEnv))};
 return{entry:{driver:kind,config,environment},inherited,host:'/fixture-home',cwd:'/fixture-cwd',expected};
});
writeFileSync(output,JSON.stringify({reference:'9e85538535c8553901351112ab02dd1dd62db5ee',sourceFiles:['apps/server/src/provider/Drivers/CodexHomeLayout.ts','apps/server/src/provider/Drivers/CodexDriver.ts','apps/server/src/provider/Drivers/ClaudeHome.ts','packages/provider-core/src/server/pathExpansion.ts'],cases},null,2)+'\n');
