#!/usr/bin/env python3
"""Run paid, live Kiro checks through ahu in a new external fixture.

Requires an existing Kiro login. Keeps evidence under /tmp, without changing
native user configuration. Tests headless execution, not helper isolation.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import re

SOURCE = Path(__file__).resolve().parents[1]


TERMINAL = {'succeeded', 'failed', 'timed_out', 'cancelled', 'capture_failed', 'boundary_violation'}
ERRORS = (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.SubprocessError)


def cleanup_tasks(ahu, owned):
    """Reconcile only a newly created fixture; return confirmed cleanup evidence."""
    evidence = []
    ids = list(owned)
    try:
        inventory = ahu('tasks')
        ids.extend(task['task_id'] for task in inventory['tasks'])
        if inventory.get('unreadable'):
            evidence.append({'status': 'inventory_unreadable'})
    except ERRORS:
        evidence.append({'status': 'inventory_unavailable'})
    for task_id in dict.fromkeys(ids):
        item = {'task_id': task_id, 'status': 'cleanup_unconfirmed'}
        try:
            current = ahu('result', task_id, check=False)
            if current.get('outcome') not in TERMINAL:
                cancelled = ahu('cancel', task_id, check=False)
                item['cancellation_accepted'] = cancelled.get('cancellation') == 'requested'
                # Observe termination even if cancellation was rejected or raced.
                current = ahu('wait', task_id, check=False)
            item['outcome'] = current.get('outcome')
            if current.get('outcome') in TERMINAL:
                item['status'] = 'terminal_confirmed'
        except ERRORS:
            pass
        evidence.append(item)
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ahu', type=Path, required=True)
    parser.add_argument('--model', default='claude-sonnet-4.5')
    parser.add_argument('--allow-widened-approvals', action='store_true', required=True)
    args = parser.parse_args()
    binary = args.ahu.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix='ahu-kiro-compatibility-', dir='/tmp')).resolve()
    repo = root / 'fixture'
    repo.mkdir()
    env = {k: v for k, v in os.environ.items() if not k.startswith(('AHU_', 'CMUX_', 'OTEL_'))}
    rows, owned = [], []
    print(f'External fixture and evidence: {root}', flush=True)

    def run(argv, check=True):
        result = subprocess.run([str(a) for a in argv], cwd=repo, env=env,
                                capture_output=True, text=True, timeout=240)
        if check and result.returncode:
            (root / 'last-error.txt').write_text(result.stderr)
            raise RuntimeError(f'{Path(argv[0]).name} exited {result.returncode}')
        return result

    def git(*argv):
        return run(['git', '-c', 'core.hooksPath=/dev/null', *argv])

    def ahu(*argv, check=True):
        result = run([binary, *argv, '--output', 'json'], check=check)
        if not result.stdout.strip():
            (root / 'last-error.txt').write_text(result.stderr)
            raise RuntimeError('ahu produced no JSON')
        return json.loads(result.stdout)

    def record(name, result, expected, extra=True):
        passed = result.get('outcome') == expected and extra
        rows.append({'case': name, 'passed': passed, 'expected': expected,
                     'outcome': result.get('outcome'), 'task_id': result.get('task_id'),
                     'attempt': result.get('attempt')})
        (root / f'{name}.json').write_text(json.dumps(result, indent=2))
        print(f"{name}: {'pass' if passed else 'FAIL'}", flush=True)

    def launch(name, prompt, *, agent='probe', timeout=120, background=False):
        path = root / f'{name}.txt'
        path.write_text(prompt)
        result = ahu(f'@{agent}', '--headless', '--allow-widened-approvals',
                     '--timeout', str(timeout), '--prompt-file', path,
                     *(['--background'] if background else []), check=False)
        if result.get('task_id'):
            owned.append(result['task_id'])
        return result

    failure = None
    try:
        git('init', '--quiet', '--template=')
        git('config', 'user.name', 'Compatibility Probe')
        git('config', 'user.email', 'probe@example.invalid')
        # Synthetic policy uses this source revision's public catalog constant.
        catalog = re.search(r'pub const CATALOG_VERSION: &str = "([^"]+)"',
                            (SOURCE / 'src/catalog.rs').read_text()).group(1)
        agents = repo / '.agents/ahu/agents'
        agents.mkdir(parents=True)
        (repo / '.agents/ahu/config.md').write_text(
            "---\nokf_version: '0.2'\ntype: ahu:project\nahu:\n"
            '  schema_version: 1\n  harness_preferences: ["kiro"]\n  model_selection: project-ranked\n'
            f'  catalog_version: {json.dumps(catalog)}\n  model_rankings:\n    kiro: [{json.dumps(args.model)}]\n---\n')
        for name, permission in [('probe', 'auto'), ('prompt-probe', 'prompt')]:
            (agents / f'{name}.md').write_text(
                f'---\nokf_version: 0.2\ntype: ahu:agent\ntitle: {name}\n'
                'description: Live Kiro compatibility probe\nstatus: stable\ntags: [agents]\n'
                f'harness: kiro\nmodel: {json.dumps(args.model)}\npermissions: {permission}\n'
                'version: 1.0.0\n---\nFollow the assigned probe. Do not delegate or change configuration.\n')
        native = repo / '.kiro/agents'
        native.mkdir(parents=True)
        (native / 'fixture-reader.json').write_text(json.dumps({
            'name': 'fixture-reader', 'description': 'Read-only fixture helper',
            'prompt': 'Read only the supplied fixture.', 'tools': ['read'], 'includePowers': False}))
        settings = repo / '.kiro/settings'
        settings.mkdir()
        keys = ['AHU_TASK_ID', 'AHU_TASK_DIR', 'AHU_EVAL_OTEL_ENDPOINT',
                'AHU_MCP_RESOURCE_ATTRIBUTES', 'OTEL_RESOURCE_ATTRIBUTES']
        (settings / 'mcp.json').write_text(json.dumps({'mcpServers': {'ahu': {
            'command': str(binary), 'args': ['mcp', 'serve'],
            'env': {key: '${' + key + '}' for key in keys}}}}))
        (repo / '.gitignore').write_text('.ahu/\n.worktrees/\n')
        (repo / 'exists.txt').write_text('RECOVERED\n')
        git('add', '.')
        git('commit', '--quiet', '-m', 'Create isolated Kiro compatibility fixture')
        run([binary, 'lock', '--update'])
        git('add', '.agents/ahu/lock.md')
        git('commit', '--quiet', '-m', 'Lock fixture context')
        case = root / 'mcp-case.md'
        case.write_text('''---
okf_version: '0.2'
type: ahu:eval-case
schema_version: 2
id: kiro-mcp-compatibility
corpus_version: 1.0.0
state:
  instruction: Call the actual ahu_agents_list MCP tool once, check whether probe is present, and write answer.json. Do not substitute shell output for the MCP call.
questions:
  registered:
    type: choice
    instructions: Is probe present in the successful MCP result?
    options:
      present: Present
      absent: Absent
expected:
  registered: present
scoring:
  registered: 1.0
  exact_match_pass_threshold: 1.0
tool_expectations:
  required_successful: [ahu_agents_list]
---
Call the tool and produce the requested answer. Do not delegate or modify source.
''')
        records = root / 'eval.jsonl'
        evaluation = ahu('eval', 'run', '--case', case, '--agent', '@probe', '--records', records,
                         '--timeout', '120', '--allow-widened-approvals', check=False)
        (root / 'eval-summary.json').write_text(json.dumps(evaluation, indent=2))
        trial = json.loads(records.read_text().splitlines()[-1])
        owned.append(trial['candidate_task_id'])
        fresh = ahu('result', owned[-1], check=False)
        metering = trial.get('trajectory', {}).get('kiro_metering', {})
        record('fresh-mcp-export-metering', fresh, 'succeeded',
               trial.get('answer_passed') is True and trial.get('tool_expectation_status') == 'pass'
               and trial.get('mcp_spans', 0) > 0 and metering.get('credit_observations', 0) >= 1
               and metering.get('observed_credits') is not None
               and metering.get('context_usage_percent') is not None)
        resume = root / 'resume.txt'
        resume.write_text('Reply RESUMED. Do not call tools or delegate.')
        ahu('resume', fresh['task_id'], '--prompt-file', resume)
        resumed = ahu('wait', fresh['task_id'], check=False)
        session = fresh.get('native_reference', {}).get('session')
        record('resume', resumed, 'succeeded', resumed.get('attempt') == 2 and session is not None
               and resumed.get('native_reference', {}).get('session') == session)
        recovery = launch('recovery', 'Use the native read tool to read absent.txt (intentionally missing), '
                          'then read exists.txt. Reply with its contents. Do not use shell, write, or delegate.')
        trajectory = recovery.get('harness', {}).get('trajectory', {})
        record('recovery', recovery, 'succeeded', trajectory.get('tool_errors', 0) >= 1)
        denial = launch('denial', 'Use the shell tool once to run printf PERMISSION_PROBE. '
                        'If denied, report that and stop. Do not use alternatives.', agent='prompt-probe')
        record('permission-denial', denial, 'failed',
               any('den' in str(b).lower() or 'permission' in str(b).lower()
                   for b in denial.get('harness', {}).get('blockers', [])))
        record('timeout', launch('timeout', 'Run sleep 30 using the shell tool, then reply DONE.',
                                 timeout=1), 'timed_out')
        background = launch('background', 'Run sleep 30 using the shell tool, then reply DONE.', background=True)
        ahu('cancel', background['task_id'], check=False)
        record('cancel', ahu('wait', background['task_id'], check=False), 'cancelled')
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        failure = type(error).__name__
        print(f'Suite stopped: {failure}; inspect external evidence', flush=True)
    finally:
        cleanup = []
        if (repo / '.agents/ahu/config.md').exists():
            cleanup = cleanup_tasks(ahu, owned)
            if any(item['status'] != 'terminal_confirmed' for item in cleanup):
                failure = failure or 'cleanup_unconfirmed'
        summary = {'schema_version': 1, 'model': args.model, 'execution_mode': 'headless',
                   'ahu_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                   'source_dirty': bool(subprocess.run(['git', 'status', '--porcelain'], cwd=SOURCE,
                                                       capture_output=True, text=True).stdout.strip()),
                   'rows': rows, 'failure': failure, 'cleanup': cleanup,
                   'native_agent_evidence': 'configuration admission only; invocation not tested',
                   'passed': failure is None and len(rows) == 6 and all(r['passed'] for r in rows)}
        for label, argv, cwd in [('harness_version', ['kiro-cli', '--version'], repo),
                                 ('source_revision', ['git', 'rev-parse', 'HEAD'], SOURCE)]:
            result = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=10)
            summary[label] = result.stdout.strip() if result.returncode == 0 else 'unknown'
        (root / 'summary.json').write_text(json.dumps(summary, indent=2))
    return 0 if summary['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
