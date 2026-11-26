"""scripts/check_audio_writes.py (#1526): a volume or mute write is named only in the guarded
files, in any Rust file of the repository, however deep. Reads sources only; reaches no device.

    python -m unittest scripts/test_check_audio_writes.py
"""
import unittest

import check_audio_writes as lint

F = 'app/src/foxglove/deep/x.rs'


def problems(line, allowed=None):
    return lint.check({F: f'fn f() {{\n    {line}\n}}\n'}, {} if allowed is None else allowed)


class AudioWrites(unittest.TestCase):
    def test_the_repo_is_clean(self):
        files = lint.rust_files()
        self.assertGreater(len(files), 500)
        # Nested folders are read too.
        self.assertTrue(any(rel.count('/') > 3 for rel in files))
        for rel in lint.ALLOWED:
            self.assertIn(rel, files)
        self.assertEqual(lint.check(files), [])

    def test_the_scanner(self):
        lint.self_test()

    def test_a_write_in_any_other_file_fails(self):
        for line in ('v.SetMute(true, &ctx);', 'IAudioEndpointVolume::SetMute(&e, true, ctx);',
                     'v.SetMasterVolumeLevelScalar(0.5, ctx);', 'send(VK_VOLUME_DOWN);'):
            p = problems(line)
            self.assertEqual(len(p), 1, line)
            self.assertTrue(p[0].startswith(F + ':2: '), p)

    def test_comments_and_other_uses_of_the_name_pass(self):
        for line in ('// v.SetMute(true, &ctx);', 'Event::SetMute { muted, reply } => {}',
                     'let p: SetMuteParams = x;'):
            self.assertEqual(problems(line), [], line)

    def test_the_allow_list_is_exact(self):
        self.assertEqual(problems('v.SetMute(m, c);', {F: 'guarded'}), [])
        self.assertEqual(len(problems('v.GetMute();', {F: 'guarded'})), 1)
        self.assertEqual(sorted(lint.ALLOWED), ['crates/hideout-services/src/mic_win.rs',
                                                'crates/info-screen/src/audio_reach.rs'])


if __name__ == '__main__':
    unittest.main()
