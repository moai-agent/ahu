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

# Authored normalized observations for invented fixture events. These are not
# native-provider parsers. A missing result remains missing at finalization.
SNAPSHOT = {
    'schema_version':1, 'harness':D['id'], 'finished':False, 'session':None,
    'terminal':False, 'terminal_evidence':None, 'failed':False, 'summary':'',
    'blockers':[], 'unknown_events':0, 'stderr_diagnostics':[],
    'stderr_unclassified_lines':0, 'stderr_informational_lines':0,
    'usage':{}, 'cost':{}, 'cost_tracking_limited':True, 'model':None,
    'model_attributes':{'ahu.harness.model.reported.coverage':'partial'},
    'skills':[], 'skill_unknown_events':0, 'skill_observation':'unavailable',
    'native':{'helpers':[], 'task_kinds':{}, 'shell_tasks':[],
        'refusals':{'depth_limit':0,'concurrency_limit':0,'budget':0},
        'reported_spawned':None, 'reported_completed':None, 'reported_max_depth':None,
        'reported_by_type':{}, 'unknown_events':0, 'terminal_seen':False, 'stream_incomplete':False},
    'native_completeness':{'complete':False,'terminated_cleanly':False,'evidence_complete':False,
        'joined':[],'unjoined':[],'violations':[],'unobserved':[],'unknown':['synthetic'], 'unknown_events':0},
    'native_blockers':[],
    'trajectory':{'source':'unsupported','coverage':'none','steps':None,'tool_calls':None,
        'completed_tool_calls':None,'tool_errors':None,'repeated_tool_calls':None,
        'repeated_tool_errors':None,'tool_recoveries':None,'malformed_events':0,
        'unclassified_events':0,'tracking_limited':True,'stream_incomplete':False},
    'write_target_candidates':[]
}

def decode(op,p):
    if op == 'decode_finish': SNAPSHOT['finished'] = True
    if op == 'decode_event':
        try: event=json.loads(p['line'])
        except Exception: event={}
        kind=event.get('fixture_event')
        if kind in ['session','result','failure']:
            if event.get('session'): SNAPSHOT['session']=event['session']
            if kind in ['result','failure']:
                SNAPSHOT['terminal']=True
                SNAPSHOT['failed']=kind == 'failure'
                SNAPSHOT['summary']=event.get('summary','')
                SNAPSHOT['native']['terminal_seen']=True
        elif kind == 'observation':
            for field in ['usage','model','trajectory']:
                if field in event: SNAPSHOT[field]=event[field]
        elif kind == 'write': SNAPSHOT['write_target_candidates'].append(event['path'])
        else: SNAPSHOT['unknown_events'] += 1
    return SNAPSHOT

def policy(p):
    root = pathlib.Path(p.get('project',p.get('cwd','/')))
    for path in [root, *root.parents]:
        candidate=path / '.fixture' / 'runtime.json'
        if candidate.is_file(): return json.loads(candidate.read_text())
    return {}

def respond(op,p):
    if op in ['decode_start','decode_event','decode_finish']: return decode(op,p)
    if op == 'probe':
        return {'version':'1.2.3','compatibility':None,'reviewed_version':False,'enforcement':{
            'harness':D['id'],'harness_version':'1.2.3','model_fixed_for_session':False,
            'gaps':['Synthetic fixture only'],'applied_controls':['exact model argument']}}
    if op == 'models':
        return {'source':'suggested','models':D['models'],'entitlement_checked':False}
    if op == 'launch':
        configured=policy(p)
        if p['permissions'] in configured.get('refuse_permissions',[]): raise ValueError('refused')
        if p['model'] in configured.get('refuse_models',[]): raise ValueError('refused')
        args = ['--model',p['model']]
        if p['permissions'] == 'auto': args += ['--auto']
        if p['permissions'] == 'accept-edits': args += ['--accept-edits']
        prompt_arg = None
        if p['mode'] != 'direct':
            args.append('--')
            prompt_arg = len(args)
            args.append('<prompt: see prompt.txt>')
        result={'program':D['program'],'args':args,'prompt_arg':prompt_arg,'native_eval_otlp':True}
        result.update(configured.get('launch_overrides',{}))
        for field in configured.get('launch_omit',[]): result.pop(field,None)
        return result
    if op == 'admission':
        configured=policy(p)
        if configured.get('refuse_admission'): raise ValueError('refused')
        if p.get('action') == 'execution_capabilities':
            return {'structured_output':True,'broker_transport':True}
        if p.get('action') == 'native_profile':
            request=p['request']
            bounded=request['policy'] == 'bounded'
            if request['policy'] not in ['disabled','bounded']: raise ValueError('policy')
            if bounded and (request['max_depth'] != 1 or request['assignment_writes']):
                raise ValueError('unsupported synthetic boundary')
            profile={'policy':request['policy'],'harness':D['id'],'args':[],
                'env':[],'env_remove':[],'helper_tools':[],
                'helper_model':(request['helper_model'] or request['session_model']) if bounded else '',
                'max_concurrent':request['max_concurrent'] if bounded else 0,
                'max_depth':1 if bounded else 0, 'enforced':[],
                'gaps':['Synthetic fixture has no native enforcement']}
            profile.update(configured.get('profile_overrides',{}))
            return profile
        return admission(p)
    if op == 'native_sources':
        if p.get('settings',{}).get('view') == 'private_fingerprints': return {'fingerprints':{}}
        if p.get('settings',{}).get('view') == 'hooks':
            return policy(p).get('hooks', {'hooks':[],'settings':[],'mcp_servers':[],'declared_plugins':[],
                    'unreadable':[],'checked':[],'unscanned_harness':D['id']})
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
        fingerprint='1'*64
        config=p.get('environment',{}).get('FIXTURE_CONFIG_DIR')
        if config:
            directory=pathlib.Path(config)
            (directory / 'probed').write_text('called')
            account=json.loads((directory / 'auth.json').read_text())
            if account.get('status') != 'available':
                return {'status':'unavailable','binding_harness':D['id'],'reason':'synthetic account unavailable'}
            fingerprint=hashlib.sha256(account['identity'].encode()).hexdigest()
        return {'status':'available','binding_harness':D['id'],'fingerprint':fingerprint,'identity_kind':'synthetic','effective_provider_address_verified':True}
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
