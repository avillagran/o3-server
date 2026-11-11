#!/usr/bin/env node
// Execute the pinned upstream's pure forge path and credential-boundary helpers.
import {readFileSync,writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const [directory,output]=process.argv.slice(2);
if(!directory||!output)throw Error('Usage: generate-forge-fixtures.mjs upstream-directory output.json');
const credentials=readFileSync(directory+'/packages/source-control-github/src/server/GitHubCredentials.ts','utf8');
const driver=readFileSync(directory+'/packages/source-control-github/src/server/driver.ts','utf8');
const policy=readFileSync(directory+'/packages/source-control-github/src/server/GitHubRequestPolicy.ts','utf8');
const secrets=readFileSync(directory+'/apps/server/src/sourceControl/sourceControlHostSecrets.ts','utf8');
const secretStart=secrets.indexOf('export function sourceControlHostSecretName(');
const secretNames=secrets.slice(secretStart,secrets.indexOf('\n}\n',secretStart)+3);
const constants=credentials.slice(credentials.indexOf('const HOSTNAME ='),credentials.indexOf('/** @public */'));
const path=driver.slice(driver.indexOf('const starredPath ='),driver.indexOf('// Only 204 means starred.'));
const start=policy.indexOf('export function gitHubApiUrls(');
const urls=policy.slice(start,policy.indexOf('\n}\n',start)+3);
if(!constants.includes('function environmentToken')||!path.includes('encodeURIComponent')||!urls.includes('api.github.com'))throw Error('Pinned source layout changed');
const code=stripTypeScriptTypes('const normalizeHost=h=>h.trim().toLowerCase();\n'+constants+path+urls+secretNames+'\nexport {starredPath};');
const api=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const hosts=['github.com','api.github.com','org.ghe.com','git.example.test','127.0.0.1:443','host:99999','localhost','example.test.','-bad.test','bad-.test','bad..test','user@host','https://github.com','github.com/path','github.com?x=1','héllo.test','GitHub.com',''];
const repos=['owner/repo','o/r.git','o/r space','o/café','o/%2e%2e','o/..','../repo','o/.','/repo','o/','o/r/extra','o/r?query#fragment',"o/!'()*~_",'o/line\nbreak'];
const envs=[{}, {GH_TOKEN:' primary ',GITHUB_TOKEN:'secondary'}, {GH_TOKEN:' ',GITHUB_TOKEN:' fallback '}, {GH_HOST:' git.example.test ',GH_ENTERPRISE_TOKEN:' enterprise ',GITHUB_ENTERPRISE_TOKEN:'other'}, {GH_HOST:'GIT.EXAMPLE.TEST',GITHUB_ENTERPRISE_TOKEN:' enterprise fallback '}, {GH_HOST:'wrong.test',GH_TOKEN:'personal',GH_ENTERPRISE_TOKEN:'enterprise'}];
writeFileSync(output,JSON.stringify({hosts:hosts.map(host=>({host,expected:api.isGitHubHost(host)})),paths:repos.map(repository=>({repository,expected:api.starredPath(repository)??null})),urls:hosts.slice(0,7).map(host=>({host,expected:api.gitHubApiUrls(host).rest})),environment:hosts.slice(0,7).flatMap(host=>envs.map(env=>({host,env,expected:api.environmentToken(host,env)}))),secretNames:['github.com','git.example.test'].map(serverHost=>({input:{kind:'github',field:'tokens',serverHost},expected:api.sourceControlHostSecretName({kind:'github',field:'tokens',serverHost})}))},null,2)+'\n');
