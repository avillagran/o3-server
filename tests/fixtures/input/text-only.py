#!/usr/bin/env python3
import json, os, sys
args=sys.argv[1:]
if os.environ.get("O3_TEXT_PEER_MARKER"): open(os.environ["O3_TEXT_PEER_MARKER"],"w").write(str(os.getpid()))
def emit(v): print(json.dumps(v),flush=True)
assert 'text-generation' in os.getcwd()
if 'exec' in args:
    raise AssertionError('Codex text-only transport must fail closed before launching')
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
