#!/usr/bin/env node
// Run the complete native scope parser and the actual grant-scope contract schema.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!output)throw Error('usage: generate-oauth-scope-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',sources={};
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const source=path=>{const value=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=crypto.createHash('sha256').update(value).digest('hex');return value;};
const moduleUrl=code=>'data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(code).replace(/from ["'](effect\/[^"']+)["']/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href))).toString('base64');
const native=await import(moduleUrl(source('packages/shared/src/oauthScope.ts')));
const auth=source('packages/contracts/src/auth.ts');
const constants=[...auth.matchAll(/(?:export )?const Auth\w+Scope = "[^"\n]+" as const;/g)].map(value=>value[0]).join('\n');
const environment=auth.match(/export const AuthEnvironmentScope = Schema\.Literals\(\[[\s\S]*?\]\);/)?.[0];
const grant=auth.match(/export const AuthGrantScope = Schema\.Literals\([\s\S]*?\n\);/)?.[0];
if(!environment||!grant)throw Error('Pinned grant schema boundary missing');
const contract=await import(moduleUrl('import * as Schema from "effect/Schema";\n'+constants+'\n'+environment+'\n'+grant));
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const scopes=contract.AuthGrantScope.literals;
const inputs=['',' ','  ','unknown','review:write','orchestration:read','orchestration:read unknown','unknown orchestration:read','orchestration:read orchestration:read','orchestration:read  filesystem:read',' orchestration:read','orchestration:read ','\ufefforchestration:read','orchestration:read\tfilesystem:read'];
for(let code=0;code<128;code++)inputs.push('unknown'+String.fromCharCode(code)+' orchestration:read');
for(const first of scopes)for(const second of scopes)inputs.push(`${second} ${first} ${second}`);
const cases=inputs.map(input=>{const parsed=native.parseOAuthScope(input);const filtered=parsed?.filter(Schema.is(contract.AuthGrantScope));return {input,expected:filtered?.length?filtered:null};});
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,grantScopes:scopes,cases},null,2)+'\n');
console.log(`${cases.length} native OAuth scope cases`);
