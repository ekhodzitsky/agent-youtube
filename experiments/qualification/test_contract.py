"""Negative contract checks complement Rust tests of actual emitted documents."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('check_contract', ROOT / 'tools/check_contract.py')
CONTRACT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CONTRACT)


class ContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.check = CONTRACT.validator()

    def test_status_rejects_unknown_state_extra_field_and_negative_budget(self):
        value = {'schema_version': 1, 'ok': True, 'command': 'status',
                 'result': {'state': 'waiting', 'next_action': 'poll_status', 'work_dir': '/job',
                            'deadline': 100, 'next_retry_at': 90, 'retry_policy': 'waiting_budget',
                            'retry_budget_remaining_seconds': 10}}
        self.assertTrue(self.check.is_valid(value))
        for key, bad in [('state', 'succeeded'), ('next_action', 'retry_now'),
                         ('retry_policy', 'unlimited'), ('retry_budget_remaining_seconds', -1),
                         ('retry_budget_remaining_seconds', None), ('secret', 'must_not_appear')]:
            with self.subTest(key=key, bad=bad):
                changed = {**value, 'result': {**value['result'], key: bad}}
                self.assertFalse(self.check.is_valid(changed))

    def test_legacy_policy_does_not_claim_a_remaining_wait_budget(self):
        value = {'schema_version': 1, 'ok': True, 'command': 'status',
                 'result': {'state': 'deadline_exceeded', 'next_action': 'inspect_preserved_artifacts',
                            'work_dir': '/job', 'deadline': 1, 'next_retry_at': None,
                            'retry_policy': 'legacy_job_deadline',
                            'retry_budget_remaining_seconds': None}}
        self.assertTrue(self.check.is_valid(value))
        value['result']['retry_budget_remaining_seconds'] = 3600
        self.assertFalse(self.check.is_valid(value))

    def test_setup_cannot_claim_inference_or_omit_integrity_checks(self):
        value = {'schema_version': 1, 'ok': True, 'command': 'setup',
                 'result': {'runtime_dir': '/runtime', 'gigastt': '/runtime/gigastt',
                            'gigastt_version': '2.22.0', 'model_dir': '/models', 'reused': False,
                            'checks': {'executable_sha256': True, 'model_files_sha256': True,
                                       'model_load': 'not_checked', 'inference': 'not_checked',
                                       'youtube_access': 'not_checked'}}}
        self.assertTrue(self.check.is_valid(value))
        value['result']['checks']['inference'] = True
        self.assertFalse(self.check.is_valid(value))
        del value['result']['checks']['inference']
        self.assertFalse(self.check.is_valid(value))

    def test_waiting_event_does_not_allow_unknown_stage(self):
        value = {'schema_version': 1, 'event': 'progress', 'stage': 'waiting', 'message': 'waiting'}
        self.assertTrue(self.check.is_valid(value))
        value['stage'] = 'unreviewed_stage'
        self.assertFalse(self.check.is_valid(value))
