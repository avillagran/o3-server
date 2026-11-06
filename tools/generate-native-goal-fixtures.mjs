#!/usr/bin/env node
// Execute only the pinned upstream's pure command parser and goal normalizer.
import {readFileSync,writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const [directory,output]=process.argv.slice(2);
if(!directory||!output)throw Error('Usage: generate-native-goal-fixtures.mjs upstream-directory output.json');
const contract=readFileSync(directory+'/packages/contracts/src/orchestrationV2.ts','utf8');
const adapter=readFileSync(directory+'/apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts','utf8');
const claude=readFileSync(directory+'/apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts','utf8');
const parser=contract.slice(contract.indexOf('export function parseGoalCommand('),contract.indexOf('/** Why T3 Code sent this `/compact`'));
const normalize=adapter.slice(adapter.indexOf('const CODEX_GOAL_STATUSES ='),adapter.indexOf('const providerGoalsEqual ='));
if(!parser.includes('function parseGoalCommand')||!normalize.includes('function providerGoalFromCodex'))throw Error('Pinned source layout changed');
const claudeHelpers=claude.slice(claude.indexOf('function textFromClaudeContent('),claude.indexOf('// In SDK mode, /goal'));
const claudeGoal=claude.slice(claude.indexOf('const CLAUDE_GOAL_SET_PREFIX ='),claude.indexOf('/** The CLI confirms a goal change'));
if(!claudeGoal.includes('function nextClaudeGoal'))throw Error('Claude goal source layout changed');
const code=stripTypeScriptTypes(parser+normalize+claudeHelpers+claudeGoal+'\nexport {providerGoalFromCodex,nextClaudeGoal};');
const api=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const texts=['/goal',' /goal edit\n','/goal Clear','/goal PAUSE','/goal resume','/goal implement\nall','/GOAL x','/goals','/goalX','/goal\u0085x','\ufeff/goal\tEDIT\n','/goal resume more'];
const commands=texts.flatMap(text=>[[],[{id:'image'}]].map(attachments=>({input:{text,attachments},expected:api.parseGoalCommand({text,attachments})})));
const goals=['active','paused','blocked','usageLimited','budgetLimited','complete'].map(status=>({objective:'  implement 1:1  ',status,tokensUsed:-5,timeUsedSeconds:-2,tokenBudget:1234}));
goals.push({objective:'\ufeffdone\ufeff',status:'complete',tokensUsed:53,timeUsedSeconds:2}, {objective:' \t ',status:'active',tokensUsed:0,timeUsedSeconds:0}, {objective:'fractional',status:'active',tokensUsed:1.5,timeUsedSeconds:2.75,tokenBudget:null});
const assistant=(text,extra={})=>({type:'assistant',parent_tool_use_id:null,message:{model:'<synthetic>',content:[{type:'text',text}]},...extra});
const current={objective:'Finish',status:'active',checks:1,lastCheck:'old feedback'};
const claudeInputs=[
 [null,assistant('Goal set: Finish')],[current,assistant('Goal set: Replacement')],
 [current,assistant('Goal set:  ')],[current,assistant('Goal cleared: Finish')],
 [current,assistant('No goal set')],[current,assistant('Goal active: Finish (3 turns)')],
 [current,assistant('Goal active: Finish (1 turn)')],
 [current,assistant('Goal active: Different (not yet evaluated)')],
 [null,assistant('\ufeffGoal set:  Work\nnow  \ufeff')],
 [current,assistant('Goal cleared: Finish',{parent_tool_use_id:'child'})],
 [current,assistant('Goal cleared: Finish',{message:{model:'haiku',content:[{type:'text',text:'Goal cleared: Finish'}]}})],
 ...[true,false].map(isSynthetic=>[current,{type:'user',isSynthetic,parent_tool_use_id:null,message:{content:'Stop hook feedback:\n[Finish]:  keep working  '}}]),
 [current,{type:'user',isSynthetic:true,parent_tool_use_id:null,message:{content:[{type:'text',text:'Stop hook feedback:\n[Finish]: '},{type:'text',text:'continue'}]}}],
 [null,{type:'user',isSynthetic:true,parent_tool_use_id:null,message:{content:'Stop hook feedback:\n[Finish]: continue'}}],
 [current,{type:'user',isSynthetic:true,parent_tool_use_id:null,message:{content:'Stop hook feedback:\n[Other]: continue'}}],
];
const claudeGoals=claudeInputs.map(([current,message])=>{const expected=api.nextClaudeGoal(current,message);return{current,message,changed:expected!==undefined,expected:expected??null};});
writeFileSync(output,JSON.stringify({source:'c55db3e9c72fdf584f4b54192fb39beb88667b59',commands,goals:goals.map(input=>({input,expected:api.providerGoalFromCodex(input)})),claudeGoals},null,2)+'\n');
