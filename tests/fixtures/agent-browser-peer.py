#!/usr/bin/python3
"""Native transport peer exercising only the isolated local MCP endpoint."""
import json,sys,urllib.request,urllib.error
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
    req=urllib.request.Request(config['url'],data=json.dumps({'jsonrpc':'2.0','id':1,'method':method,'params':params or {}}).encode(),headers=headers or config.get('headers',config.get('http_headers')))
    return json.load(urllib.request.urlopen(req,timeout=10))['result']
def check(text):
    if text=='disabled':
        assert not config or config.get('enabled') is False
        return
    assert config and config.get('url')
    try:
        call('tools/list',headers={'Authorization':'Bearer wrong'})
        raise AssertionError('anonymous accepted')
    except urllib.error.HTTPError as e: assert e.code==404
    assert call('initialize')['serverInfo']['name']=='t3-code'
    assert any(t['name']=='preview_open' for t in call('tools/list')['tools'])
    def tool(name,args):
        return call('tools/call',{'name':name,'arguments':args})
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
