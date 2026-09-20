import hashlib
import hmac
import unittest
from receiver import verify


class SignatureTest(unittest.TestCase):
    def test_exact_bytes_identity_date_and_generation(self):
        secret = bytes(range(32))
        keys = {"1": secret.hex()}
        body = b'{"deliveryId":"test"}'
        headers = {"x-layrs-timestamp": "1000", "x-layrs-delivery": "test", "x-layrs-key-generation": "1"}
        headers["x-layrs-signature"] = "v1=" + hmac.new(secret, b"1000.test." + body, hashlib.sha256).hexdigest()
        self.assertTrue(verify(keys, headers, body, 1000))
        self.assertFalse(verify(keys, headers, body + b" ", 1000))
        self.assertFalse(verify(keys, headers, body, 1400))
        self.assertFalse(verify(keys, {**headers, "x-layrs-delivery": "other"}, body, 1000))
        self.assertFalse(verify(keys, {**headers, "x-layrs-key-generation": "2"}, body, 1000))


if __name__ == "__main__":
    unittest.main()
