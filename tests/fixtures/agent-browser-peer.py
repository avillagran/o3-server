#!/usr/bin/python3
"""Native transport peer exercising provider-owned credentials on the shared MCP endpoint."""
import json,sys,urllib.request,urllib.error,os
session=None
owner=None
claude='-p' in sys.argv
config=None
if claude and '--mcp-config' in sys.argv:
    raw=sys.argv[sys.argv.index('--mcp-config')+1]
    config=(json.loads(raw) if raw.startswith('{') else json.load(open(raw)))['mcpServers'].get('t3-code')
    if config:
        import os,stat
        secret=config['headers']['Authorization'].encode()
        assert secret not in open('/proc/self/cmdline','rb').read(), 'MCP credential exposed in argv'
        assert stat.S_IMODE(os.stat(raw).st_mode)==0o600
        assert stat.S_IMODE(os.stat(os.path.dirname(raw)).st_mode)==0o700
def call(method,params=None,headers=None):
    global session,owner
    current=json.dumps(config,sort_keys=True)
    if current!=owner:
        session=None;owner=current
    h=dict(headers if headers is not None else config.get('headers',config.get('http_headers')))
    h.update({'Content-Type':'application/json','Accept':'application/json','Mcp-Protocol-Version':'2025-06-18'})
    if session and method!='initialize':h['Mcp-Session-Id']=session
    req=urllib.request.Request(config['url'],data=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params or {}}).encode(),headers=h)
    with urllib.request.urlopen(req,timeout=10) as response:
        if method=='initialize':session=response.headers['Mcp-Session-Id']
        reply=json.load(response)
        assert 'result' in reply,reply
        return reply['result']
def check(text):
    text,_,capture=text.partition('|')
    assert config and config.get('url')
    try:
        call('tools/list',headers={'Authorization':'Bearer wrong'})
        raise AssertionError('anonymous accepted')
    except urllib.error.HTTPError as e: assert e.code==401
    if not session or owner!=json.dumps(config,sort_keys=True):
        assert call('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'native-peer','version':'1'}})['serverInfo']['name']=='T3 Code'
    tools=call('tools/list')['tools']
    assert any(t['name']=='t3_thread_configure' for t in tools)
    assert any(t['name']=='preview_open' for t in tools)==(text=='enabled')
    def tool(name,args):
        return call('tools/call',{'name':name,'arguments':args})
    configured=tool('t3_thread_configure',{})
    assert not configured['isError'],configured
    listed=tool('t3_thread_list',{})
    assert not listed['isError'],listed
    assert listed['structuredContent']['currentThreadId']=='t',listed
    assert listed['structuredContent']['projectId']=='p',listed
    assert not tool('t3_thread_organize',{'action':'pin'})['isError']
    status=tool('t3_worktree_status',{})
    assert not status['isError'],status
    assert status['structuredContent']['attached'] is False,status
    assert status['structuredContent']['defaultStartFromOrigin'] is True,status
    assert tool('t3_project_create',{'title':'forbidden'})['structuredContent']['code']=='capability_denied'

    # Parent limits override a forged mode-limit header and wire arguments.
    denied=tool('t3_thread_organize',{'threadId':'higher','action':'pin'})
    assert denied['isError'],denied
    assert denied['structuredContent']['code']=='runtime_mode_escalation_denied',denied
    if capture:
        with os.fdopen(os.open(capture,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600),'w') as f:
            json.dump({'config':config,'session':session,'pid':os.getpid()},f)
    if text=='disabled':return
    assert tool('preview_open',{'url':'file:///etc/passwd'})['isError']
    opened=tool('preview_open',{'url':'http://localhost:1234'})
    assert not opened['isError'],opened
    tab=json.loads(opened['content'][0]['text'])['tabId']
    assert tool('preview_navigate',{'tabId':'foreign','url':'http://localhost'})['isError']
    assert not tool('preview_navigate',{'tabId':tab,'url':'http://localhost:1234/next'})['isError']
    assert not tool('preview_evaluate',{'tabId':tab,'expression':'1+1'})['isError']
    assert not tool('preview_status',{})['isError']
for line in sys.stdin:
    f=json.loads(line)
    if claude:
        if f['type']=='control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}),flush=True);continue
        if f['type']!='user':continue
        check(f['message']['content'][0]['text'])
        print(json.dumps({'type':'system','subtype':'init','session_id':'native'}),flush=True)
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'broker-verified'}}}),flush=True)
        print(json.dumps({'type':'result','subtype':'success','session_id':'native'}),flush=True)
    else:
        m,i=f.get('method'),f.get('id')
        if i is None:continue
        result={}
        if m=='model/list':result={'data':[{'model':'test','displayName':'Test','isDefault':True}]}
        if m in ('thread/start','thread/resume','thread/fork'):
            config=f['params']['config']['mcp_servers.t3-code'];result={'thread':{'id':'native'}}
        if m=='turn/start':check(f['params']['input'][0]['text']);result={'turn':{'id':'turn'}}
        print(json.dumps({'id':i,'result':result}),flush=True)
        if m=='turn/start':
            print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'native','turnId':'turn','delta':'broker-verified'}}),flush=True)
            print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}}),flush=True)
