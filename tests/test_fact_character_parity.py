"""ASCII fast path preserves Unicode categories, key consent and byte bounds."""
import json
import unittest
import unicodedata

from observatory.operation_facts import credential_key,safe_value


class FactCharacterParityTests(unittest.TestCase):
    def test_ascii_text_keeps_only_tab_newline_and_printable_characters(self):
        raw=''.join(map(chr,range(128)))
        expected=''.join(char for char in raw if char in '\t\n' or 32<=ord(char)<127)
        self.assertEqual(safe_value(raw),(expected,False))
        self.assertEqual(safe_value('ordinary ASCII\twith layout\nand punctuation = 42'),
                         ('ordinary ASCII\twith layout\nand punctuation = 42',True))

    def test_every_unicode_code_point_retains_existing_category_and_utf8_rules(self):
        # Bounded windows avoid turning this into a payload-limit test. This
        # oracle states the Unicode contract independently of the fast path.
        for start in range(0,0x110000,512):
            raw=''.join(map(chr,range(start,min(start+512,0x110000))))
            if 0xD800<=start<0xE000:
                expected=('[invalid Unicode omitted]',False)
            else:
                text=''.join(char for char in raw if char in '\t\n' or unicodedata.category(char) not in {'Cc','Cf'})
                expected=(text,text==raw)
            actual=safe_value(raw)
            if actual!=expected:self.fail('Unicode window U+%04X changed: %r != %r'%(start,actual,expected))

    def test_control_obfuscated_credential_keys_remain_redacted(self):
        controls=[*map(chr,range(32)),chr(127),'\u0085','\u00ad','\u200b','\u202e','\ufeff']
        for control in controls:
            with self.subTest(codepoint=ord(control)):
                key='api'+control+'_key'
                self.assertTrue(credential_key(key))
                text,complete=safe_value(json.dumps({key:'SYNTHETIC_PRIVATE_SECRET'}))
                self.assertFalse(complete)
                self.assertNotIn('SYNTHETIC_PRIVATE_SECRET',text)
                value,complete=safe_value({key:'SYNTHETIC_PRIVATE_SECRET'})
                self.assertFalse(complete)
                self.assertNotIn('SYNTHETIC_PRIVATE_SECRET',json.dumps(value))


if __name__=='__main__':unittest.main()
