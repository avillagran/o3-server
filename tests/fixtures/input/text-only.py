#!/usr/bin/env python3
import json, os, sys
args=sys.argv[1:]
def emit(v): print(json.dumps(v),flush=True)
assert 'text-generation' in os.getcwd()
if 'exec' in args:
    assert args == ['-a','never','exec','--ephemeral','--skip-git-repo-check','--sandbox','read-only','--disable','shell_tool','--disable','apply_patch_freeform','-c','web_search="disabled"','--json','--model','test','-'],args
    assert os.environ['CODEX_HOME'].startswith(os.getcwd())
    sys.stdin.read()
    emit({'type':'item.completed','item':{'type':'agent_message','text':'Safe title'}})
else:
    assert args[:10] == ['-p','--input-format','stream-json','--output-format','stream-json','--verbose','--include-partial-messages','--permission-prompt-tool','stdio','--allow-dangerously-skip-permissions'],args
    assert args[10:] == ['--model','test','--permission-mode','default','--no-session-persistence','--strict-mcp-config','--mcp-config','{}','--setting-sources','user,project,local','--tools','','--disable-slash-commands','--setting-sources','','--settings','{"disableAllHooks":true}'],args
    assert os.listdir('.') == [],os.listdir('.')
    for line in sys.stdin:
        v=json.loads(line)
        if v['type']=='control_request':
            emit({'type':'control_response','response':{'subtype':'success','request_id':'initialize','response':{}}})
        else:
            emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'Safe title'}}})
            emit({'type':'result','subtype':'success','is_error':False,'result':'Safe title'})
