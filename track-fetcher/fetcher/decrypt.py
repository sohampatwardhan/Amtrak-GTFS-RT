"""Decrypt the maps.amtrak.com train feed.

The ciphertext is the same envelope the public ``amtk`` crate opens: the last 88 characters are
a base64 private-key blob, and the rest is the base64 body. Both use AES-128-CBC with PKCS#7
padding. The key is PBKDF2-HMAC-SHA1 (salt ``9a3686ac``, 1000 iterations, 16 bytes) and the IV
is the fixed maps value. The private-key blob is decrypted with the public maps key; the body
is decrypted with the private key (the text before ``|``).
"""

from __future__ import annotations

import base64

from cryptography.hazmat.primitives import hashes, padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

PBKDF2_SALT = bytes.fromhex("9a3686ac")
AES_IV = bytes.fromhex("c6eb2f7f5c4740c1a2f708fefd947d39")
PUBLIC_KEY = b"69af143c-e8cf-47f8-bf09-fc1f61e5cc33"
_PRIVATE_KEY_LENGTH = 88


class DecryptError(ValueError):
    """The maps payload is not a valid encrypted train feed."""


def derive_key(password: bytes) -> bytes:
    """16-byte AES key for ``password``, matching the maps feed."""
    kdf = PBKDF2HMAC(algorithm=hashes.SHA1(), length=16, salt=PBKDF2_SALT, iterations=1000)
    return kdf.derive(password)


def _decrypt(ciphertext: bytes, password: bytes) -> bytes:
    key = derive_key(password)
    decryptor = Cipher(algorithms.AES(key), modes.CBC(AES_IV)).decryptor()
    padded = decryptor.update(ciphertext) + decryptor.finalize()
    unpadder = padding.PKCS7(128).unpadder()
    try:
        return unpadder.update(padded) + unpadder.finalize()
    except ValueError as error:
        raise DecryptError("ciphertext did not decrypt") from error


def decrypt_private_key_blob(blob: str) -> str:
    """Decrypt an 88-character private-key blob with the public maps key."""
    return _decrypt(_b64(blob.strip()), PUBLIC_KEY).decode("utf-8")


def decrypt_payload(data: str) -> str:
    """Return the UTF-8 plaintext of one maps ``getTrainsData`` body."""
    text = data.strip()
    if len(text) <= _PRIVATE_KEY_LENGTH:
        raise DecryptError("payload is too short to contain a private key")
    body, encrypted_key = text[: -_PRIVATE_KEY_LENGTH], text[-_PRIVATE_KEY_LENGTH :]
    try:
        key_plain = _decrypt(_b64(encrypted_key), PUBLIC_KEY).decode("utf-8")
        private_key = key_plain.split("|", 1)[0]
        if not private_key:
            raise DecryptError("private key is empty")
        return _decrypt(_b64(body), private_key.encode("utf-8")).decode("utf-8")
    except (UnicodeDecodeError, DecryptError) as error:
        raise DecryptError("payload did not decrypt") from error


def _b64(text: str) -> bytes:
    try:
        return base64.b64decode(text, validate=True)
    except Exception as error:  # noqa: BLE001 — base64 raises several decode errors
        raise DecryptError("payload is not base64") from error
