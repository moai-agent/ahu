#!/usr/bin/env python3
"""Validate this frozen corpus using only Python's standard library.

This intentionally accepts the JSON subset of YAML used in these OKF files,
not arbitrary YAML. Semantic labels still require independent review.
"""
import argparse
from collections import Counter, defaultdict
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent
QUESTIONS = {'action', 'claim', 'validation'}
LEVELS = {0, 0.5, 1}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f'duplicate JSON key: {key}')
        result[key] = value
    return result


def parse_json(text):
    def invalid_number(value):
        raise ValueError(f'non-finite JSON number: {value}')
    return json.loads(text, object_pairs_hook=unique_object,
                      parse_constant=invalid_number)


def read_json(path):
    require(path.is_file() and not path.is_symlink(), f'not a regular file: {path}')
    return parse_json(path.read_text(encoding='utf-8'))


def read_okf(path):
    require(path.is_file() and not path.is_symlink(), f'not a regular file: {path}')
    raw = path.read_bytes()
    require(len(raw) <= 256 * 1024, f'oversized OKF: {path}')
    text = raw.decode('utf-8')
    require(text.startswith('---\n'), f'missing front matter: {path}')
    front, sep, purpose = text[4:].partition('\n---\n')
    require(sep and 0 < len(purpose.strip().encode()) <= 8192, f'invalid body: {path}')
    return parse_json(front)


def safe_id(value):
    return isinstance(value, str) and re.fullmatch(r'[A-Za-z0-9._-]{1,64}', value)


def bounded_json(value, depth=0):
    require(depth <= 8, 'JSON nesting limit exceeded')
    if isinstance(value, str):
        require(len(value.encode()) <= 2048, 'JSON string exceeds ahu limit')
    elif isinstance(value, list):
        require(len(value) <= 64, 'JSON array exceeds ahu limit')
        for item in value:
            bounded_json(item, depth + 1)
    elif isinstance(value, dict):
        require(len(value) <= 64, 'JSON object exceeds ahu limit')
        for key, item in value.items():
            require(len(key.encode()) <= 128, 'JSON key exceeds ahu limit')
            bounded_json(item, depth + 1)
    else:
        require(value is None or type(value) in (int, float, bool), 'unsupported JSON value')


def validate_case(case):
    require(set(case) == {'okf_version', 'type', 'schema_version', 'id', 'corpus_version',
                         'state', 'questions', 'expected', 'scoring', 'rubric'}, 'case fields differ')
    require(case['okf_version'] == '0.2' and case['type'] == 'ahu:eval-case'
            and type(case['schema_version']) is int and case['schema_version'] == 2, 'case schema differs')
    require(safe_id(case['id']) and case['corpus_version'] == '1.0.0', 'case identity differs')
    state = case['state']
    require(set(state) == {'user_request', 'evidence'}, 'state fields differ')
    require(isinstance(state['user_request'], str) and state['user_request'].strip(), 'missing request')
    require(isinstance(state['evidence'], list) and len(state['evidence']) >= 3
            and all(isinstance(p, str) and len(p.split()) >= 30 for p in state['evidence']),
            'evidence must contain several substantial paragraphs')
    count = len((' '.join([state['user_request'], *state['evidence']])).split())
    require(250 <= count <= 500, f'{case["id"]}: state word count {count} outside 250..500')
    bounded_json(state)
    for name in ('questions', 'expected', 'rubric'):
        require(set(case[name]) == QUESTIONS, f'{name} question coverage differs')
    require(set(case['scoring']) == QUESTIONS | {'exact_match_pass_threshold'}, 'scoring keys differ')
    require(all(type(case['scoring'][q]) in (int, float) and case['scoring'][q] == 1
                for q in QUESTIONS)
            and type(case['scoring']['exact_match_pass_threshold']) in (int, float)
            and case['scoring']['exact_match_pass_threshold'] == 0.75,
            'this corpus uses equal weights and a 0.75 threshold (all three exact choices required)')
    for q, question in case['questions'].items():
        require(set(question) == {'type', 'instructions', 'options'} and question['type'] == 'choice',
                f'{q}: not a typed choice')
        require(isinstance(question['instructions'], str)
                and 0 < len(question['instructions'].encode()) <= 2000, 'invalid question instructions')
        options = question['options']
        require(set(options) == {'A', 'B', 'C'}, f'{q}: option keys differ')
        require(all(isinstance(v, str) and 0 < len(v.encode()) <= 500 for v in options.values()),
                f'{q}: empty or oversized option')
        require(len(set(options.values())) == 3, f'{q}: duplicate option text')
        require(isinstance(case['expected'][q], str) and case['expected'][q] in options,
                f'{q}: expected is not an allowed key')
        rubric = case['rubric'][q]
        require(isinstance(rubric, str) and 0 < len(rubric.encode()) <= 1000, f'{q}: invalid rubric')
        for level in ('Does not satisfy (0):', 'Partially satisfies (0.5):', 'Fully satisfies (1):'):
            require(level in rubric, f'{q}: missing level {level}')
        require(not re.search(r'\b(?:option|key)\s+[ABC]\b', rubric), f'{q}: rubric names an answer key')
    bounded_json(case['questions'])
    return count


def validate_reference(case, ref):
    require(set(ref) == {'frozen_answer', 'expected_criterion_scores', 'rationale', 'critical_false_accept'},
            'reference fields differ')
    for field in ref:
        require(isinstance(ref[field], dict) and set(ref[field]) == QUESTIONS,
                f'{field}: criterion coverage differs')
    for q in QUESTIONS:
        answer = ref['frozen_answer'][q]
        require(isinstance(answer, str) and answer in case['questions'][q]['options'],
                f'{q}: frozen answer is not an allowed option key')
        score = ref['expected_criterion_scores'][q]
        require(type(score) in (int, float) and score in LEVELS, f'{q}: invalid reference level')
        require((answer == case['expected'][q]) == (score == 1),
                f'{q}: best-answer and fully-satisfies label disagree')
        require(isinstance(ref['rationale'][q], str) and len(ref['rationale'][q].split()) >= 10,
                f'{q}: missing substantive rationale')
        flag = ref['critical_false_accept'][q]
        require(type(flag) is bool and (not flag or score < 1), f'{q}: invalid critical flag')


def judge_input(case, frozen_answer):
    """Positive allowlist: no id, expected keys, labels, identity, or trace."""
    return deepcopy({'case_state': case['state'], 'questions': case['questions'],
                     'frozen_answer': frozen_answer, 'rubric': case['rubric']})


def load_suite(base, expected_count, prefix):
    suite = read_okf(base / 'suite.md')
    require(set(suite) == {'okf_version', 'type', 'schema_version', 'id', 'suite_version', 'cases'},
            'suite fields differ')
    require(suite['okf_version'] == '0.2' and suite['type'] == 'ahu:eval-suite'
            and type(suite['schema_version']) is int and suite['schema_version'] == 1,
            'ahu suite schema must be 1')
    expected_id = 'decision-grading-pilot' if prefix == '' else 'decision-grading-confirmatory'
    require(suite['id'] == expected_id and suite['suite_version'] == '1.0.0', 'suite identity differs')
    require(len(suite['cases']) == expected_count, 'incorrect suite count')
    refs = read_json(base / 'reference.json')
    cases = {}
    paths = set()
    for entry in suite['cases']:
        require(set(entry) == {'path', 'weight'} and type(entry['weight']) in (int, float)
                and entry['weight'] == 1, 'suite entry differs')
        name = entry['path']
        require(isinstance(name, str) and re.fullmatch(re.escape(prefix) + r'[a-z0-9-]+\.md', name),
                'unexpected suite path')
        require(name not in paths, 'duplicate case path')
        paths.add(name)
        path = base / name
        require(not path.parent.is_symlink(), 'case directory is a symlink')
        case = read_okf(path)
        validate_case(case)
        cid = case['id']
        require(path.stem == cid and cid not in cases, 'case id mismatch or duplication')
        require(cid in refs, 'case missing reference')
        validate_reference(case, refs[cid])
        cases[cid] = case
    require(set(refs) == set(cases), 'extra or missing references')
    found = {p.relative_to(base).as_posix() for p in (base / prefix).glob('*.md') if p.name != 'suite.md'}
    require(found == paths, 'unlisted case files')
    return cases, refs


def validate_corpus(root=ROOT):
    confirm, refs = load_suite(root, 12, 'cases/')
    pilots, pilot_refs = load_suite(root / 'pilot', 2, '')
    require(not (confirm.keys() & pilots.keys()), 'pilot leaks into confirmatory suite')
    manifest = read_json(root / 'manifest.json')
    require(manifest['schema_version'] == 1 and manifest['corpus_version'] == '1.0.0', 'manifest version differs')
    domains = manifest['domains']
    require(len(domains) == 6 and all(len(ids) == 2 for ids in domains.values()), 'need six pairs of domains')
    domain_ids = [cid for ids in domains.values() for cid in ids]
    require(len(set(domain_ids)) == 12 and set(domain_ids) == set(confirm), 'domain coverage differs')
    require(set(manifest['pilots']) == set(pilots) and len(manifest['pilots']) == 2, 'pilot inventory differs')
    by_score = Counter(); by_question = defaultdict(Counter)
    by_key = defaultdict(Counter); by_position = defaultdict(Counter); best_keys = Counter()
    critical = 0
    for cid, case in confirm.items():
        for q in QUESTIONS:
            ref = refs[cid]; score = ref['expected_criterion_scores'][q]; answer = ref['frozen_answer'][q]
            by_score[score] += 1; by_question[q][score] += 1
            by_key[answer][score] += 1
            by_position[list(case['questions'][q]['options']).index(answer)][score] += 1
            best_keys[case['expected'][q]] += 1
            critical += ref['critical_false_accept'][q]
    require(len({sum(ref['expected_criterion_scores'].values()) for ref in refs.values()}) >= 3,
            'case-level mixtures do not vary')
    require(by_score == Counter({0: 12, 0.5: 12, 1: 12}), 'reference levels unbalanced')
    for table in (by_question, by_key, by_position):
        require(len(table) == 3 and all(row == Counter({0: 4, 0.5: 4, 1: 4}) for row in table.values()),
                'question, selected-key, or selected-position shortcut detected')
    require(best_keys == Counter({'A': 12, 'B': 12, 'C': 12}), 'best-answer keys unbalanced')
    require(0 < critical < 24, 'critical subset missing or includes every non-full judgment')
    lock = read_json(root / 'freeze.json')
    required = {'suite.md', 'reference.json', 'manifest.json', 'pilot/suite.md', 'pilot/reference.json'}
    required |= {f'cases/{cid}.md' for cid in confirm} | {f'pilot/{cid}.md' for cid in pilots}
    require(set(lock) == required, 'freeze inventory differs')
    for relative, expected_digest in lock.items():
        require(hashlib.sha256((root / relative).read_bytes()).hexdigest() == expected_digest,
                f'frozen file changed: {relative}; obtain review and version the corpus')
    return confirm, refs, pilots, pilot_refs


def self_test(case, ref):
    tests = 0
    def rejected(fn):
        nonlocal tests
        try:
            fn()
        except (ValueError, TypeError, KeyError):
            tests += 1
        else:
            raise ValueError('mutation unexpectedly accepted')
    rejected(lambda: parse_json('{"a":1,"a":2}'))
    rejected(lambda: parse_json('{"a":NaN}'))
    for field, key, value in [('frozen_answer', 'action', 'Z'),
                              ('frozen_answer', 'action', True),
                              ('expected_criterion_scores', 'claim', True),
                              ('expected_criterion_scores', 'claim', 0.25),
                              ('rationale', 'claim', ''),
                              ('critical_false_accept', 'action', 'false')]:
        bad = deepcopy(ref); bad[field][key] = value
        rejected(lambda: validate_reference(case, bad))
    bad = deepcopy(ref); del bad['frozen_answer']['claim']
    rejected(lambda: validate_reference(case, bad))
    bad = deepcopy(case); bad['state']['evidence'] = ['Too short.']
    rejected(lambda: validate_case(bad))
    bad = deepcopy(case); bad['questions']['action']['options']['A'] = ''
    rejected(lambda: validate_case(bad))
    # Alter hidden metadata: the judge input must remain byte-for-byte identical.
    before = judge_input(case, ref['frozen_answer'])
    poisoned = deepcopy(case)
    for field in ('expected', 'id', 'scoring', 'identity', 'trace'):
        poisoned[field] = 'HIDDEN_SENTINEL'
    after = judge_input(poisoned, ref['frozen_answer'])
    require(before == after and 'HIDDEN_SENTINEL' not in json.dumps(after), 'judge-input boundary failed')
    require(set(after) == {'case_state', 'questions', 'frozen_answer', 'rubric'}, 'judge-input allowlist failed')
    tests += 1
    return tests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true', help='exercise malformed inputs and blinding invariants')
    parser.add_argument('--judge-input', metavar='CASE_ID', help='emit only this case’s permitted judge input as JSON')
    args = parser.parse_args()
    try:
        confirm, refs, pilots, pilot_refs = validate_corpus()
        if args.judge_input:
            cases = {**confirm, **pilots}; references = {**refs, **pilot_refs}
            require(args.judge_input in cases, 'unknown case id')
            print(json.dumps(judge_input(cases[args.judge_input], references[args.judge_input]['frozen_answer']), indent=2))
            return
        print('PASS: 12 confirmatory cases / 36 judgments; 2 separate pilots / 6 judgments.')
        print('PASS: 12 each of 0, 0.5, 1; each question, selected key and displayed position has 4 of each level.')
        print('PASS: allowed keys, references, six domain pairs, schemas and frozen SHA-256 inventory.')
        if args.self_test:
            cid = next(iter(confirm))
            print(f'PASS: {self_test(confirm[cid], refs[cid])} malformed-input and judge-boundary checks.')
    except (ValueError, TypeError, KeyError, OSError) as exc:
        print(f'FAIL: {exc}', file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
