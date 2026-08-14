# Dependency Security Audit

**Result:** WARNINGS — review required; this is not a clean audit

## Audit context

| Field | Value |
|---|---|
| Mode | release |
| Completed | 2026\-08\-14T22:25:44\.326300Z |
| Project revision | 20b693d3e09fa2e03e11ec6c2d893f806f26af60 |
| Inventory fingerprint | 725e38374fc90bc622980a32a3657224e6bf276cb9cbf4e91a787bc1103d3fc8 |
| Inventory completeness | complete |
| Stable exit code | 0 |

## Report links

- [Machine-readable JSON](latest.json)
- [Immutable JSON evidence](audit-20260814T222544Z.json)

## Source availability

| Source | State | Provenance | Diagnostic |
|---|---|---|---|
| cargo\-audit | ok | not recorded | — |
| github | not\_applicable | [source](https://api.github.com/advisories) | — |
| github | not\_applicable | [source](https://api.github.com/advisories) | — |
| github | not\_applicable | [source](https://api.github.com/advisories) | — |
| github | not\_applicable | [source](https://api.github.com/advisories) | — |
| github | not\_applicable | [source](https://api.github.com/advisories) | — |
| github | not\_applicable | [source](https://api.github.com/advisories) | — |
| github | ok | [source](https://api.github.com/advisories) | — |
| github | ok | [source](https://api.github.com/advisories) | — |
| github | ok | [source](https://api.github.com/advisories) | — |
| govulncheck | not\_applicable | not recorded | ecosystem not present |
| kev | ok | [source](https://www.cisa.gov/sites/default/files/feeds/known_exploited_vulnerabilities.json) | — |
| npm\-audit | not\_applicable | not recorded | ecosystem not present |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | not\_applicable | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| nvd | ok | [source](https://services.nvd.nist.gov/rest/json/cves/2.0) | — |
| osv | ok | [source](https://api.osv.dev/v1) | — |
| pip\-audit | not\_applicable | not recorded | ecosystem not present |

## Inventory

Resolved packages: **375**.

## Blocking findings (0)

None.

## Warnings (9)

### atomic\-polyfill — RUSTSEC\-2023\-0089

- Package: pkg:cargo/atomic\-polyfill@1\.0\.3
- Installed version: 1\.0\.3
- Severity: unknown
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: The author has archived the GitHub repository and mentions deprecation in project&\#x27;s \[README\]\(https://github\.com/embassy\-rs/atomic\-polyfill/blob/48e55c166684f37af0b00fbee5a0809b1a2bae8e/README\.md\)\.  \#\# Possible alternatives   \* \[portable\-atomic\]\(https://crates\.io/crates/portable\-atomic\)
- [Advisory evidence 1](https://crates.io/crates/atomic-polyfill)
- [Advisory evidence 2](https://github.com/embassy-rs/atomic-polyfill/commit/48e55c166684f37af0b00fbee5a0809b1a2bae8e)
- [Advisory evidence 3](https://rustsec.org/advisories/RUSTSEC-2023-0089.html)
### derivative — RUSTSEC\-2024\-0388

- Package: pkg:cargo/derivative@2\.2\.0
- Installed version: 2\.2\.0
- Severity: unknown
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: The \[\`derivative\`\]\(https://crates\.io/crates/derivative\) crate is no longer maintained\. Consider using any alternative, for instance: \- \[derive\_more\]\(https://crates\.io/crates/derive\_more\) \- \[derive\-where\]\(https://crates\.io/crates/derive\-where\) \- \[educe\]\(https://crates\.io/crates/educe\)
- [Advisory evidence 1](https://crates.io/crates/derivative)
- [Advisory evidence 2](https://github.com/mcarton/rust-derivative/issues/117)
- [Advisory evidence 3](https://rustsec.org/advisories/RUSTSEC-2024-0388.html)
### fxhash — RUSTSEC\-2025\-0057

- Package: pkg:cargo/fxhash@0\.2\.1
- Installed version: 0\.2\.1
- Severity: unknown
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: The fxhash crate is no longer maintained\.  The repository is stale and owner is no longer active on GitHub\.  Please take a look at \[rustc\-hash\]\(https://github\.com/rust\-lang/rustc\-hash\) instead\. \`\`\`
- [Advisory evidence 1](https://crates.io/crates/fxhash)
- [Advisory evidence 2](https://github.com/cbreeden/fxhash/issues/20)
- [Advisory evidence 3](https://rustsec.org/advisories/RUSTSEC-2025-0057.html)
### gcc — RUSTSEC\-2025\-0121

- Package: pkg:cargo/gcc@0\.3\.55
- Installed version: 0\.3\.55
- Severity: unknown
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: The \`gcc\` crate is deprecated and no longer actively maintained\. If you rely on this crate, consider switching to a recommended alternative\.  \#\# Recommended alternatives  \- \[\`cc\`\]\(https://crates\.io/crates/cc\)
- [Advisory evidence 1](https://crates.io/crates/gcc)
- [Advisory evidence 2](https://rustsec.org/advisories/RUSTSEC-2025-0121.html)
### rust\-crypto — RUSTSEC\-2016\-0005

- Package: pkg:cargo/rust\-crypto@0\.2\.36
- Installed version: 0\.2\.36
- Severity: unknown
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: non\_blocking\_severity
- Fixed versions: 0\.2\.37\-0
- Advisory summary: The \`rust\-crypto\` crate has not seen a release or GitHub commit since 2016, and its author is unresponsive\.  \*NOTE: The \(old\) \`rust\-crypto\` crate \(with hyphen\) should not be confused with similarly named \(new\) \[RustCrypto GitHub Org\] \(without hyphen\)\. The GitHub Org is actively maintained\.\*  We recommend you switch to one of the following crates instead, depending on which algorithms you need:  \- \[dalek\-cryptography GitHub Org\]:   \- Key agreement: \[\`x25519\-dalek\`\]   \- Signature algorithms: \[\`ed25519\-dalek\`\] \- \[\`ring\`\]:   \- AEAD algorithms: AES\-GCM, ChaCha20Poly1305   \- Digest algorithms: SHA\-256, SHA\-384, SHA\-512, SHA\-512/256 \(legacy: SHA\-1\)   \- HMAC   \- Key agreement: ECDH \(P\-256, P\-384\), X25519   \- Key derivation: HKDF   \- Password hashing: PBKDF2   \- Signature algorithms: ECDSA \(P\-256, P\-384\), Ed25519, RSA \(PKCS\#1v1\.5, PSS\) \- \[RustCrypto GitHub Org\]:   \- AEAD algorithms: \[\`aes\-gcm\`\], \[\`aes\-gcm\-siv\`\], \[\`aes\-siv\`\], \[\`chacha20poly1305\`\], \[\`xsalsa20poly1305\`\]   \- Block ciphers: \[\`aes\`\], \[\`cast5\`\], \[\`des\`\]   \- Digest algorithms: \[\`sha2\`\], \[\`sha3\`\], \[\`blake2\`\], \[\`ripemd160\`\]     \(legacy: \[\`sha\-1\`\], \[\`md\-5\`\]\)   \- Key derivation: \[\`hkdf\`\]   \- MACs: \[\`cmac\`\], \[\`hmac\`\], \[\`pmac\`\], \[\`poly1305\`\]   \- Password hashing: \[\`pbkdf2\`\]   \- Stream ciphers: \[\`aes\-ctr\`\], \[\`chacha20\`\], \[\`hc\-256\`\], \[\`salsa20\`\] \- \[\`secp256k1\`\]:   \- Key agreement: ECDH \(secp256k1 only\)   \- Signature algorithms: ECDSA \(secp256k1 only\) \- \[\`orion\`\]:   \- AEAD algorithms: ChaCha20Poly1305 \(IETF version\), XChaCha20Poly1305   \- Digest algorithms: SHA\-512, BLAKE2b   \- Key derivation: HKDF   \- MACs: HMAC, Poly1305   \- Password hashing: PBKDF2   \- Stream ciphers: ChaCha20 \(IETF version\), XChaCha20  \[dalek\-cryptography GitHub Org\]: https://github\.com/dalek\-cryptography \[RustCrypto GitHub Org\]: https://github\.com/RustCrypto \[\`aes\`\]: https://crates\.io/crates/aes \[\`aes\-ctr\`\]: https://crates\.io/crates/aes\-ctr \[\`aes\-gcm\`\]: https://crates\.io/crates/aes\-gcm \[\`aes\-gcm\-siv\`\]: https://crates\.io/crates/aes\-gcm\-siv \[\`aes\-siv\`\]: https://crates\.io/crates/aes\-siv \[\`blake2\`\]: https://crates\.io/crates/blake2 \[\`cast5\`\]: https://crates\.io/crates/cast5 \[\`chacha20\`\]: https://crates\.io/crates/chacha20 \[\`chacha20poly1305\`\]: https://crates\.io/crates/chacha20poly1305 \[\`cmac\`\]: https://crates\.io/crates/cmac \[\`des\`\]: https://crates\.io/crates/des \[\`ed25519\-dalek\`\]: https://crates\.io/crates/ed25519\-dalek \[\`hc\-256\`\]: https://crates\.io/crates/hc\-256 \[\`hkdf\`\]: https://crates\.io/crates/hkdf \[\`hmac\`\]: https://crates\.io/crates/hmac \[\`pbkdf2\`\]: https://crates\.io/crates/pbkdf2 \[\`pmac\`\]: https://crates\.io/crates/pmac \[\`poly1305\`\]: https://crates\.io/crates/poly1305 \[\`ring\`\]: https://crates\.io/crates/ring \[\`ripemd160\`\]: https://crates\.io/crates/ripemd160 \[\`salsa20\`\]: https://crates\.io/crates/salsa20 \[\`secp256k1\`\]: https://crates\.io/crates/secp256k1 \[\`sha\-1\`\]: https://crates\.io/crates/sha\-1 \[\`sha2\`\]: https://crates\.io/crates/sha2 \[\`sha3\`\]: https://crates\.io/crates/sha3 \[\`x25519\-dalek\`\]: https://crates\.io/crates/x25519\-dalek \[\`xsalsa20poly1305\`\]: https://crates\.io/crates/xsalsa20poly1305 \[\`orion\`\]: https://crates\.io/crates/orion
- [Advisory evidence 1](https://crates.io/crates/rust-crypto)
- [Advisory evidence 2](https://github.com/DaGenix/rust-crypto/issues/440)
- [Advisory evidence 3](https://rustsec.org/advisories/RUSTSEC-2016-0005.html)
### rust\-crypto — RUSTSEC\-2022\-0011

- Package: pkg:cargo/rust\-crypto@0\.2\.36
- Installed version: 0\.2\.36
- Severity: critical
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: The following Rust program demonstrates some strangeness in AES encryption \- if you have an immutable key slice and then operate on that slice, you get different encryption output than if you operate on a copy of that key\.  For these functions, we expect that extending a 16 byte key to a 32 byte key by repeating it gives the same encrypted data, because the underlying rust\-crypto functions repeat key data up to the necessary key size for the cipher\.  \`\`\`rust use crypto::\{     aes, blockmodes, buffer,     buffer::\{BufferResult, ReadBuffer, WriteBuffer\},     symmetriccipher, \};  fn encrypt\(     key: &amp;\[u8\],     iv: &amp;\[u8\],     data: &amp;str, \) \-&gt; Result&lt;String, symmetriccipher::SymmetricCipherError&gt; \{     let mut encryptor =         aes::cbc\_encryptor\(aes::KeySize::KeySize256, key, iv, blockmodes::PkcsPadding\);      let mut encrypted\_data = Vec::&lt;u8&gt;::new\(\);     let mut read\_buffer = buffer::RefReadBuffer::new\(data\.as\_bytes\(\)\);     let mut buffer = \[0; 4096\];     let mut write\_buffer = buffer::RefWriteBuffer::new\(&amp;mut buffer\);      loop \{         let result = encryptor\.encrypt\(&amp;mut read\_buffer, &amp;mut write\_buffer, true\)?;          encrypted\_data\.extend\(             write\_buffer                 \.take\_read\_buffer\(\)                 \.take\_remaining\(\)                 \.iter\(\)                 \.copied\(\),         \);          match result \{             BufferResult::BufferUnderflow =&gt; break,             BufferResult::BufferOverflow =&gt; \{\}         \}     \}      Ok\(hex::encode\(encrypted\_data\)\) \}  fn working\(\) \{     let data = &quot;data&quot;;     let iv = \[         0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0xFE,         0xFF,     \];     let key = \[         0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,         0x0F,     \];     // The copy here makes the code work\.     let key\_copy = key;     let key2: Vec&lt;u8&gt; = key\_copy\.iter\(\)\.cycle\(\)\.take\(32\)\.copied\(\)\.collect\(\);     println\!\(&quot;key1:\{\} key2: \{\}&quot;, hex::encode\(&amp;key\), hex::encode\(&amp;key2\)\);      let x1 = encrypt\(&amp;key, &amp;iv, data\)\.unwrap\(\);     println\!\(&quot;X1: \{\}&quot;, x1\);      let x2 = encrypt\(&amp;key2, &amp;iv, data\)\.unwrap\(\);     println\!\(&quot;X2: \{\}&quot;, x2\);      assert\_eq\!\(x1, x2\); \}  fn broken\(\) \{     let data = &quot;data&quot;;     let iv = \[         0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0xFE,         0xFF,     \];     let key = \[         0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,         0x0F,     \];     // This operation shouldn&\#x27;t affect the contents of key at all\.     let key2: Vec&lt;u8&gt; = key\.iter\(\)\.cycle\(\)\.take\(32\)\.copied\(\)\.collect\(\);     println\!\(&quot;key1:\{\} key2: \{\}&quot;, hex::encode\(&amp;key\), hex::encode\(&amp;key2\)\);      let x1 = encrypt\(&amp;key, &amp;iv, data\)\.unwrap\(\);     println\!\(&quot;X1: \{\}&quot;, x1\);      let x2 = encrypt\(&amp;key2, &amp;iv, data\)\.unwrap\(\);     println\!\(&quot;X2: \{\}&quot;, x2\);      assert\_eq\!\(x1, x2\); \}  fn main\(\) \{     working\(\);     broken\(\); \} \`\`\`  The output from this program:  \`\`\`shell      Running \`target/host/debug/rust\-crypto\-test\` key1:000102030405060708090a0b0c0d0e0f key2: 000102030405060708090a0b0c0d0e0f000102030405060708090a0b0c0d0e0f X1: 90462bbe32965c8e7ea0addbbed4cddb X2: 90462bbe32965c8e7ea0addbbed4cddb key1:000102030405060708090a0b0c0d0e0f key2: 000102030405060708090a0b0c0d0e0f000102030405060708090a0b0c0d0e0f X1: 26e847e5e7df1947bf82a650548a7d5b X2: 90462bbe32965c8e7ea0addbbed4cddb thread &\#x27;main&\#x27; panicked at &\#x27;assertion failed: \`\(left == right\)\`   left: \`&quot;26e847e5e7df1947bf82a650548a7d5b&quot;\`,  right: \`&quot;90462bbe32965c8e7ea0addbbed4cddb&quot;\`&\#x27;, src/main\.rs:83:5 \`\`\`  Notably, the X1 key in the \`broken\(\)\` test changes every time after rerunning the program\.
- [Advisory evidence 1](https://crates.io/crates/rust-crypto)
- [Advisory evidence 2](https://github.com/DaGenix/rust-crypto)
- [Advisory evidence 3](https://github.com/DaGenix/rust-crypto/issues/476)
- [Advisory evidence 4](https://github.com/advisories/GHSA-jp3w-3q88-34cf)
- [Advisory evidence 5](https://rustsec.org/advisories/RUSTSEC-2022-0011.html)
### rustc\-serialize — RUSTSEC\-2022\-0004

- Package: pkg:cargo/rustc\-serialize@0\.3\.25
- Installed version: 0\.3\.25
- Severity: medium
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: When parsing JSON using \`json::Json::from\_str\`, there is no limit to the depth of the stack, therefore deeply nested objects can cause a stack overflow, which aborts the process\.  Example code that triggers the vulnerability is  \`\`\`rust fn main\(\) \{     let \_ = rustc\_serialize::json::Json::from\_str\(&amp;&quot;\[0,\[&quot;\.repeat\(10000\)\); \} \`\`\`  \[serde\]\(https://crates\.io/crates/serde\) is recommended as a replacement to rustc\_serialize\.
- [Advisory evidence 1](https://crates.io/crates/rustc-serialize)
- [Advisory evidence 2](https://github.com/advisories/GHSA-2226-4v3c-cff8)
- [Advisory evidence 3](https://github.com/rust-lang-deprecated/rustc-serialize)
- [Advisory evidence 4](https://rustsec.org/advisories/RUSTSEC-2022-0004.html)
### rustc\-serialize — RUSTSEC\-2025\-0025

- Package: pkg:cargo/rustc\-serialize@0\.3\.25
- Installed version: 0\.3\.25
- Severity: unknown
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: no\_authoritative\_fix
- Fixed versions: none identified
- Advisory summary: \`rustc\-serialize\` will no longer be maintained as declared by the developer\. By fuzzing the package, we can identify multiple vulnerabilities\. The project has been archived and cannot submit issues\. The developer has recommended using the \`serde\` crate instead\.
- [Advisory evidence 1](https://crates.io/crates/rustc-serialize)
- [Advisory evidence 2](https://github.com/rust-lang-deprecated/rustc-serialize/issues)
- [Advisory evidence 3](https://rustsec.org/advisories/RUSTSEC-2025-0025.html)
### time — RUSTSEC\-2020\-0071

- Package: pkg:cargo/time@0\.1\.45
- Installed version: 0\.1\.45
- Severity: medium
- Dependency scope: unknown
- KEV status: not identified in CISA KEV
- Reachability: not\_assessed
- Decision reasons: non\_blocking\_severity
- Fixed versions: 0\.2\.0, 0\.2\.1, 0\.2\.2, 0\.2\.23, 0\.2\.3, 0\.2\.4, 0\.2\.5, 0\.2\.6
- Advisory summary: \#\#\# Impact  The affected functions set environment variables without synchronization\. On Unix\-like operating systems, this can crash in multithreaded programs\. Programs may segfault due to dereferencing a dangling pointer if an environment variable is read in a different thread than the affected functions\. This may occur without the user&\#x27;s knowledge, notably in the Rust standard library or third\-party libraries\.  The affected functions from time 0\.2\.7 through 0\.2\.22 are:  \- \`time::UtcOffset::local\_offset\_at\` \- \`time::UtcOffset::try\_local\_offset\_at\` \- \`time::UtcOffset::current\_local\_offset\` \- \`time::UtcOffset::try\_current\_local\_offset\` \- \`time::OffsetDateTime::now\_local\` \- \`time::OffsetDateTime::try\_now\_local\`  The affected functions in time 0\.1 \(all versions\) are:  \- \`time::at\_utc\` \- \`time::at\` \- \`time::now\` \- \`time::tzset\`  Non\-Unix targets \(including Windows and wasm\) are unaffected\.  \#\#\# Patches  Pending a proper fix, the internal method that determines the local offset has been modified to always return \`None\` on the affected operating systems\. This has the effect of returning an \`Err\` on the \`try\_\*\` methods and \`UTC\` on the non\-\`try\_\*\` methods\.  Users and library authors with time in their dependency tree should perform \`cargo update\`, which will pull in the updated, unaffected code\.  Users of time 0\.1 do not have a patch and should upgrade to an unaffected version: time 0\.2\.23 or greater or the 0\.3 series\.  \#\#\# Workarounds  A possible workaround for crates affected through the transitive dependency in \`chrono\`, is to avoid using the default \`oldtime\` feature dependency of the \`chrono\` crate by disabling its \`default\-features\` and manually specifying the required features instead\.  \#\#\#\# Examples:  \`Cargo\.toml\`:    \`\`\`toml chrono = \{ version = &quot;0\.4&quot;, default\-features = false, features = \[&quot;serde&quot;\] \} \`\`\`  \`\`\`toml chrono = \{ version = &quot;0\.4\.22&quot;, default\-features = false, features = \[&quot;clock&quot;\] \} \`\`\`  Commandline:    \`\`\`bash cargo add chrono \-\-no\-default\-features \-F clock \`\`\`  Sources:    \- \[chronotope/chrono\#602 \(comment\)\]\(https://github\.com/chronotope/chrono/issues/602\#issuecomment\-1242149249\)    \- \[vityafx/serde\-aux\#21\]\(https://github\.com/vityafx/serde\-aux/issues/21\)
- [Advisory evidence 1](https://crates.io/crates/time)
- [Advisory evidence 2](https://crates.io/crates/time/0.2.23)
- [Advisory evidence 3](https://github.com/advisories/GHSA-wcg3-cvx6-7396)
- [Advisory evidence 4](https://github.com/time-rs/time)
- [Advisory evidence 5](https://github.com/time-rs/time/issues/293)
- [Advisory evidence 6](https://github.com/time-rs/time/security/advisories/GHSA-wcg3-cvx6-7396)
- [Advisory evidence 7](https://nvd.nist.gov/vuln/detail/CVE-2020-26235)
- [Advisory evidence 8](https://rustsec.org/advisories/RUSTSEC-2020-0071.html)

## Excluded findings (0)

None.

## Unclassified findings (0)

None.

## Unmatched decisions (0)

None.

## Remediation and acceptance

- **atomic\-polyfill / RUSTSEC\-2023\-0089:** Inherited through the locked geojson dependency graph\. The release retains scratch\-image, non\-root, loopback\-default, peer\-policy, exact\-image smoke\-test, SBOM, and zero\-match Grype controls, with the review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **derivative / RUSTSEC\-2024\-0388:** Inherited through locked gtfs\-structures; no maintained same\-chain replacement is available\. The release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **fxhash / RUSTSEC\-2025\-0057:** Inherited through the locked scraper/selectors chain; no maintained same\-chain replacement is available\. The release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **gcc / RUSTSEC\-2025\-0121:** Deprecated build dependency inherited through locked rust\-crypto\. The release runtime is a scratch image and retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **rust\-crypto / RUSTSEC\-2016\-0005:** The unmaintained crate is a direct legacy dependency with no released common safe version\. Replacement requires upstream API work; the release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **rust\-crypto / RUSTSEC\-2022\-0011:** No patched rust\-crypto release exists and reachability is not established\. Replacement requires upstream API work; the release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **rustc\-serialize / RUSTSEC\-2022\-0004:** Inherited through locked rust\-crypto with no patched same\-chain release\. The release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **rustc\-serialize / RUSTSEC\-2025\-0025:** The unmaintained crate is inherited through locked rust\-crypto\. Replacement requires upstream API work; the release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
- **time / RUSTSEC\-2020\-0071:** Affected time 0\.1\.45 is inherited through locked rust\-crypto; migration requires replacing that upstream chain\. The release retains the controls and review triggers documented in the companion Markdown record\.
  Risk acceptance: Repository owner explicitly accepted this exact nine\-warning v0\.2\.0 risk set on 2026\-08\-14 through 2026\-11\-12\.
