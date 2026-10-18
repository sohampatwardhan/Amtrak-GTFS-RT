"""Offline decrypt checks against the live maps key schedule."""

from fetcher.decrypt import PUBLIC_KEY, DecryptError, decrypt_payload, decrypt_private_key_blob, derive_key

# Produced with the maps public key, salt, and IV. The last 88 characters are the private-key blob.
_PRIVATE_KEY = (
    "ni5O7BmLf33E1+IdsIKDu0fjHi7FB2WrQMo+/7G52e8VlmRWt3iLUEddH5RdTeE08/"
    "N3WWL0xcFY114Pj8f4MA=="
)
_BODY = "LtxCIma0apZtLzD+cHjRgA=="
# Vector published by the amtk crate's own tests.
_AMTK_KEY_BLOB = (
    "CXgnq/gU8YfogvSlywDWApL5s0sE6SKd9M4Ky0zuH2F+BWkSTSXSMNFyEAIb3MpKwOgIZX3uGuNIUzUpevqlWQ=="
)


def test_live_key_vector_opens_the_body_with_the_appended_private_key():
    assert len(_PRIVATE_KEY) == 88
    assert decrypt_payload(_BODY + _PRIVATE_KEY) == '{"ok":true}'


def test_public_key_derivation_matches_the_amtk_crate():
    assert derive_key(PUBLIC_KEY) == bytes(
        [
            0x07,
            0x1E,
            0x28,
            0x3E,
            0x78,
            0x2B,
            0x88,
            0x27,
            0x39,
            0x6D,
            0x64,
            0x86,
            0xDF,
            0xB8,
            0x70,
            0x27,
        ]
    )
    assert (
        decrypt_private_key_blob(_PRIVATE_KEY)
        == "11111111-2222-3333-4444-555555555555|2099-01-01T00:00:00.000Z"
    )
    assert (
        decrypt_private_key_blob(_AMTK_KEY_BLOB)
        == "7bb0fd35-d2a0-4ca5-9e1d-f6f1a1a8eed7|2021-06-05T07:12:21.000Z"
    )


def test_a_short_or_garbage_payload_is_rejected():
    try:
        decrypt_payload("not-a-payload")
    except DecryptError:
        pass
    else:
        raise AssertionError("expected DecryptError")
