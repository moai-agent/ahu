# Development fixture only. No actual native provider behavior is implemented.
import os, sys, pathlib, hashlib
assert sys.argv[1:] == ['--ahu-harness-runtime-protocol=1']
assert 'HOME' not in os.environ and 'PATH' not in os.environ
D = FIXTURE['descriptor']

def parse(text):
    fields = {}
    instructions = text
    if text.startswith('---\n') and '\n---\n' in text[4:]:
        header, instructions = text[4:].split('\n---\n', 1)
        for line in header.splitlines():
            if ':' in line:
                key, value = line.split(':', 1)
                fields[key.strip()] = value.strip().strip('"\'')
    return {'instructions':instructions, 'native_model':fields.get('model'),
            'native_settings':{k:v for k,v in fields.items() if k != 'model'}}

def admission(p):
    return {'allowed':True,'decision':'admitted','guarded_checks':'inspected','conformance':'untested',
            'helper_profile':None,'invocation_profile':None,
            'evidence':{'harness':D['id'],'native_agent':None,'profile':None,
                'cli_version':p.get('version'),'version_checked':True,'components':[],
                'gaps':['Synthetic fixture only'],'conformance':'untested',
                'headless':{'allowed':True,'reasons':[],'evidence_digest':'0'*64},'next_action':''}}

def respond(op,p):
    if op == 'probe':
        return {'version':'1.2.3','compatibility':None,'reviewed_version':False,'enforcement':{
            'harness':D['id'],'harness_version':'1.2.3','model_fixed_for_session':False,
            'gaps':['Synthetic fixture only'],'applied_controls':['exact model argument']}}
    if op == 'models':
        return {'source':'suggested','models':D['models'],'entitlement_checked':False}
    if op == 'launch':
        args = ['--model',p['model']]
        if p['permissions'] == 'auto': args += ['--auto']
        if p['permissions'] == 'accept-edits': args += ['--accept-edits']
        prompt_arg = None
        if p['mode'] != 'direct':
            prompt_arg = len(args)
            args.append('<prompt: see prompt.txt>')
        return {'program':D['program'],'args':args,'prompt_arg':prompt_arg,'native_eval_otlp':True}
    if op == 'admission': return admission(p)
    if op == 'native_sources':
        if p.get('settings',{}).get('view') == 'hooks':
            return {'hooks':[],'settings':[],'mcp_servers':[],'declared_plugins':[],
                    'unreadable':[],'checked':[],'unscanned_harness':D['id']}
        if p.get('action') == 'parse':
            if p['format'] != FIXTURE['format']: raise ValueError('unsupported format')
            return parse(p['text'])
        if p.get('action') == 'discover':
            root = pathlib.Path(p['project_dir'])
            result=[]
            for path in sorted((root / FIXTURE['source_dir']).glob('*.md')):
                if path.is_symlink(): continue
                text=path.read_text(); parsed=parse(text)
                result.append({'name':path.stem,'path':str(path.relative_to(root)),
                    'format':FIXTURE['format'],'native_model':parsed['native_model'],
                    'blockers':[],'preserved_fields':list(parsed['native_settings']),
                    'description':parsed['native_settings'].get('description','')})
            return result
    if op == 'setup':
        path=pathlib.Path(p['project']) / '.fixture' / 'settings.json'
        before=hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
        return {'publication':'host_transaction_required','changes':[{
            'path':str(path),'expected_before_sha256':before,'text':'{"configured":true}\n'}]}
    if op == 'auth':
        return {'status':'available','binding_harness':D['id'],'fingerprint':'1'*64,'identity_kind':'synthetic','effective_provider_address_verified':True}
    if op == 'budget': return {'supported':False}
    raise ValueError('unsupported operation')

sequence = 0
for line in sys.stdin:
    request=json.loads(line)
    sequence += 1
    assert request['protocol_version'] == 1 and request['id'] == sequence
    response={'protocol_version':1,'id':sequence}
    try: response['result']=respond(request['operation'],request['params'])
    except Exception: response['error']='failed'
    print(json.dumps(response),flush=True)
