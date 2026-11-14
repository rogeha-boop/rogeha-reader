//! Digital (certificate) signatures: sign a PDF with a PKCS#12 identity and check the
//! signatures in a PDF.
//!
//! A digital signature is the standard PDF kind (`/Filter /Adobe.PPKLite`,
//! `/SubFilter /adbe.pkcs7.detached`): a detached CMS / PKCS#7 `SignedData` over every byte of
//! the file except the signature's own slot. It proves two things a picture of a signature
//! cannot: which certificate signed, and that the signed bytes have not changed since.
//!
//! How signing works here (no private MuPDF signing API is involved):
//! 1. a signature field is added whose `/Contents` is a block of zeros and whose `/ByteRange`
//!    holds placeholder numbers, and the document is saved (incrementally when possible, so
//!    earlier signatures stay valid);
//! 2. the slot is located in the saved bytes, the real byte range is written in, everything
//!    outside the slot is hashed with SHA-256, and the hash is signed with the identity's key;
//! 3. the resulting CMS structure is written into the slot as hex.
//!
//! Limits of this version: RSA keys only (the common case; no elliptic-curve certificates yet);
//! no timestamp authority; when checking, the signer's certificate chain is **not** validated
//! against trusted authorities, so the result says whether the signature is intact and which
//! certificate made it, not whether that certificate deserves trust.

use std::path::{Path, PathBuf};

use mupdf::pdf::{PdfObject, PdfWriteOptions};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use rsa::sha2::{Digest, Sha256, Sha384, Sha512};
use rsa::{Generate, PrivateKey, PublicKey};
use sha1::Sha1;

use crate::engine::{Engine, Result, err};

/// Room reserved for the signature, in bytes (twice that in hex). Enough for a 4096-bit key
/// with a chain of several certificates.
const SLOT_BYTES: usize = 12_000;
const RANGE_PLACEHOLDER: &str = "[0 1000000000 1000000000 1000000000]";

// ------------------------------------------------------------------------- DER

mod asn1 {
    //! The small part of DER needed for certificates and CMS.

    pub fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let n = body.len();
        if n < 0x80 {
            out.push(n as u8);
        } else {
            let bytes = n.to_be_bytes();
            let skip = bytes.iter().take_while(|b| **b == 0).count();
            out.push(0x80 | (bytes.len() - skip) as u8);
            out.extend_from_slice(&bytes[skip..]);
        }
        out.extend_from_slice(body);
        out
    }

    pub fn seq(parts: &[&[u8]]) -> Vec<u8> {
        tlv(0x30, &parts.concat())
    }

    pub fn set(parts: &[&[u8]]) -> Vec<u8> {
        tlv(0x31, &parts.concat())
    }

    pub fn oid(dotted: &str) -> Vec<u8> {
        let arcs: Vec<u64> = dotted.split('.').map(|a| a.parse().unwrap()).collect();
        let mut body = vec![(arcs[0] * 40 + arcs[1]) as u8];
        for &arc in &arcs[2..] {
            let mut chunk = vec![(arc & 0x7f) as u8];
            let mut rest = arc >> 7;
            while rest > 0 {
                chunk.push((rest & 0x7f) as u8 | 0x80);
                rest >>= 7;
            }
            chunk.reverse();
            body.extend(chunk);
        }
        tlv(0x06, &body)
    }

    /// A non-negative INTEGER from big-endian bytes.
    pub fn uint(bytes: &[u8]) -> Vec<u8> {
        let trimmed: &[u8] = match bytes.iter().position(|b| *b != 0) {
            Some(i) => &bytes[i..],
            None => &[0],
        };
        let mut body = Vec::with_capacity(trimmed.len() + 1);
        if trimmed[0] & 0x80 != 0 {
            body.push(0);
        }
        body.extend_from_slice(trimmed);
        tlv(0x02, &body)
    }

    pub const NULL: [u8; 2] = [0x05, 0x00];

    /// One element: its tag, its contents, and the whole encoding.
    #[derive(Clone, Copy)]
    pub struct Tlv<'a> {
        pub tag: u8,
        pub body: &'a [u8],
        pub raw: &'a [u8],
    }

    /// Read one element from the front of `input`; returns it and what follows.
    pub fn read(input: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
        let (&tag, rest) = input.split_first()?;
        let (&first, rest) = rest.split_first()?;
        let (len, rest) = if first < 0x80 {
            (first as usize, rest)
        } else {
            let n = (first & 0x7f) as usize;
            if n == 0 || n > 4 || rest.len() < n {
                return None;
            }
            (rest[..n].iter().fold(0usize, |acc, b| acc << 8 | *b as usize), &rest[n..])
        };
        if rest.len() < len {
            return None;
        }
        let header = input.len() - rest.len();
        Some((Tlv { tag, body: &rest[..len], raw: &input[..header + len] }, &rest[len..]))
    }

    /// All elements directly inside `body`.
    pub fn children(mut body: &[u8]) -> Option<Vec<Tlv<'_>>> {
        let mut out = Vec::new();
        while !body.is_empty() {
            let (item, rest) = read(body)?;
            out.push(item);
            body = rest;
        }
        Some(out)
    }
}

use asn1::{NULL, Tlv, oid, seq, set, tlv};

const OID_DATA: &str = "1.2.840.113549.1.7.1";
const OID_SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
const OID_CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
const OID_MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
const OID_SIGNING_TIME: &str = "1.2.840.113549.1.9.5";
const OID_RSA: &str = "1.2.840.113549.1.1.1";
const OID_SHA256_RSA: &str = "1.2.840.113549.1.1.11";
const OID_SHA1: &str = "1.3.14.3.2.26";
const OID_SHA256: &str = "2.16.840.1.101.3.4.2.1";
const OID_SHA384: &str = "2.16.840.1.101.3.4.2.2";
const OID_SHA512: &str = "2.16.840.1.101.3.4.2.3";
const OID_COMMON_NAME: &str = "2.5.4.3";
const OID_ORGANIZATION: &str = "2.5.4.10";
const OID_KEY_USAGE: &str = "2.5.29.15";
const OID_BASIC_CONSTRAINTS: &str = "2.5.29.19";

// ------------------------------------------------------------------------- time

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// (year, month, day, hour, minute, second) in UTC.
fn civil(unix: u64) -> (i64, i64, i64, u64, u64, u64) {
    let (days, rem) = ((unix / 86_400) as i64, unix % 86_400);
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

fn from_civil(y: i64, m: i64, d: i64, h: u64, mi: u64, s: u64) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (y.div_euclid(400), y.rem_euclid(400));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    ((era * 146_097 + doe - 719_468) * 86_400) as u64 + h * 3600 + mi * 60 + s
}

/// DER time: UTCTime up to 2049, GeneralizedTime after (as X.509 requires).
fn der_time(unix: u64) -> Vec<u8> {
    let (y, m, d, h, mi, s) = civil(unix);
    if y < 2050 {
        tlv(0x17, format!("{:02}{m:02}{d:02}{h:02}{mi:02}{s:02}Z", y % 100).as_bytes())
    } else {
        tlv(0x18, format!("{y:04}{m:02}{d:02}{h:02}{mi:02}{s:02}Z").as_bytes())
    }
}

fn parse_der_time(t: Tlv) -> Option<u64> {
    let s = std::str::from_utf8(t.body).ok()?;
    let digits = |r: std::ops::Range<usize>| s.get(r)?.parse::<u64>().ok();
    let (y, rest) = match t.tag {
        0x17 => {
            let yy = digits(0..2)? as i64;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, 2)
        }
        0x18 => (digits(0..4)? as i64, 4),
        _ => return None,
    };
    Some(from_civil(
        y,
        digits(rest..rest + 2)? as i64,
        digits(rest + 2..rest + 4)? as i64,
        digits(rest + 4..rest + 6)?,
        digits(rest + 6..rest + 8)?,
        digits(rest + 8..rest + 10).unwrap_or(0),
    ))
}

/// A date for people: `2026-11-14 09:30 UTC`.
pub fn format_time(unix: u64) -> String {
    let (y, m, d, h, mi, _) = civil(unix);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02} UTC")
}

// ------------------------------------------------------------------ certificates

/// The parts of an X.509 certificate this module needs.
struct Cert<'a> {
    issuer: &'a [u8],
    serial: &'a [u8],
    subject: &'a [u8],
    spki: &'a [u8],
    not_before: u64,
    not_after: u64,
}

impl<'a> Cert<'a> {
    fn parse(der: &'a [u8]) -> Option<Self> {
        let (cert, _) = asn1::read(der)?;
        let tbs = asn1::children(cert.body)?.first().copied()?;
        let mut fields = asn1::children(tbs.body)?.into_iter().peekable();
        if fields.peek()?.tag == 0xa0 {
            fields.next(); // version
        }
        let serial = fields.next()?;
        let _signature_alg = fields.next()?;
        let issuer = fields.next()?;
        let validity = asn1::children(fields.next()?.body)?;
        let subject = fields.next()?;
        let spki = fields.next()?;
        Some(Self {
            issuer: issuer.raw,
            serial: serial.raw,
            subject: subject.raw,
            spki: spki.raw,
            not_before: parse_der_time(*validity.first()?)?,
            not_after: parse_der_time(*validity.get(1)?)?,
        })
    }

    /// The value of one attribute (by OID) of the subject name, e.g. the common name.
    fn subject_attribute(&self, wanted: &str) -> Option<String> {
        let wanted = oid(wanted);
        let (name, _) = asn1::read(self.subject)?;
        for rdn in asn1::children(name.body)? {
            for attribute in asn1::children(rdn.body)? {
                let parts = asn1::children(attribute.body)?;
                if parts.first()?.raw == wanted.as_slice() {
                    let value = parts.get(1)?;
                    return Some(match value.tag {
                        // BMPString: UTF-16 big endian.
                        0x1e => String::from_utf16_lossy(
                            &value.body.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>(),
                        ),
                        _ => String::from_utf8_lossy(value.body).into_owned(),
                    });
                }
            }
        }
        None
    }

    fn display_name(&self) -> String {
        self.subject_attribute(OID_COMMON_NAME)
            .or_else(|| self.subject_attribute(OID_ORGANIZATION))
            .unwrap_or_else(|| "(unnamed certificate)".to_owned())
    }
}

// --------------------------------------------------------------------- identity

/// A signing identity: a private key and its certificate (plus any chain certificates).
pub struct Identity {
    key: PrivateKey,
    /// DER certificates, the signer's own first.
    certs: Vec<Vec<u8>>,
}

impl Identity {
    /// Open a `.p12` / `.pfx` file.
    pub fn from_pkcs12(data: &[u8], password: &str) -> Result<Self> {
        use p12_keystore::{KeyStore, Pkcs12ImportPolicy};
        let store = KeyStore::from_pkcs12(data, password, Pkcs12ImportPolicy::Relaxed)
            .map_err(|_| "This certificate file could not be opened. Check the password.".to_owned())?;
        let (_, chain) = store
            .private_key_chain()
            .ok_or("This file has no private key, so it cannot be used to sign.")?;
        let key = PrivateKey::from_pkcs8_der(chain.key().as_der())
            .map_err(|_| "Only RSA certificates are supported in this version.".to_owned())?;
        let spki = key.to_public_key().to_public_key_der().map_err(err)?;
        let mut certs: Vec<Vec<u8>> = chain.certs().iter().map(|c| c.as_der().to_vec()).collect();
        // The signer's certificate is the one that carries this key.
        let own = certs
            .iter()
            .position(|der| Cert::parse(der).is_some_and(|c| c.spki == spki.as_bytes()))
            .ok_or("This file has no certificate for its private key.")?;
        certs.swap(0, own);
        Ok(Self { key, certs })
    }

    /// The name on the certificate.
    pub fn name(&self) -> String {
        Cert::parse(&self.certs[0]).map_or_else(String::new, |c| c.display_name())
    }

    /// Until when the certificate is valid (Unix time).
    pub fn valid_until(&self) -> u64 {
        Cert::parse(&self.certs[0]).map_or(0, |c| c.not_after)
    }

    /// Make a new self-signed identity and return it as a password-protected `.p12` file.
    ///
    /// A self-signed certificate shows that the document has not changed and that the same
    /// key signed it, but other people's readers will report the signer as unverified until
    /// they choose to trust the certificate.
    pub fn create_self_signed(name: &str, organization: &str, password: &str, years: u32) -> Result<Vec<u8>> {
        use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKey as StoredKey, PrivateKeyChain};
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter the name to put on the certificate".into());
        }
        if password.is_empty() {
            return Err("Choose a password to protect the certificate file".into());
        }
        let key = PrivateKey::generate();
        let spki = key.to_public_key().to_public_key_der().map_err(err)?;

        let attribute = |o: &str, value: &str| set(&[&seq(&[&oid(o), &tlv(0x0c, value.as_bytes())])]);
        let mut rdns = Vec::new();
        if !organization.trim().is_empty() {
            rdns.push(attribute(OID_ORGANIZATION, organization.trim()));
        }
        rdns.push(attribute(OID_COMMON_NAME, name));
        let subject = tlv(0x30, &rdns.concat());

        let issued = now().saturating_sub(3600); // tolerate clocks that are a little behind
        let expires = issued + u64::from(years.max(1)) * 365 * 86_400;
        let mut serial = Sha256::digest([spki.as_bytes(), &issued.to_be_bytes()].concat())[..16].to_vec();
        serial[0] &= 0x7f; // positive
        serial[0] |= 0x40; // and not shortened by leading zeros
        let algorithm = seq(&[&oid(OID_SHA256_RSA), &NULL]);
        // keyUsage: digitalSignature + nonRepudiation (critical); not a certificate authority.
        let extensions = seq(&[
            &seq(&[&oid(OID_KEY_USAGE), &[0x01, 0x01, 0xff], &tlv(0x04, &[0x03, 0x02, 0x06, 0xc0])]),
            &seq(&[&oid(OID_BASIC_CONSTRAINTS), &tlv(0x04, &seq(&[]))]),
        ]);
        let tbs = seq(&[
            &tlv(0xa0, &asn1::uint(&[2])), // version 3
            &tlv(0x02, &serial),
            &algorithm,
            &subject, // issuer = subject: self-signed
            &seq(&[&der_time(issued), &der_time(expires)]),
            &subject,
            spki.as_bytes(),
            &tlv(0xa3, &extensions),
        ]);
        let signature = key.pkcs1v15_sign::<Sha256>(&tbs).map_err(err)?;
        let cert = seq(&[&tbs, &algorithm, &tlv(0x03, &[&[0u8][..], &signature].concat())]);

        let pkcs8 = key.to_pkcs8_der().map_err(err)?;
        let key_id = Sha256::digest(spki.as_bytes())[..20].to_vec();
        let chain = PrivateKeyChain::new(
            key_id,
            StoredKey::from_der(pkcs8.as_bytes()).map_err(err)?,
            [Certificate::from_der(&cert).map_err(err)?],
        );
        let mut store = KeyStore::new();
        store.add_entry(name, KeyStoreEntry::PrivateKeyChain(chain));
        store.writer(password).write().map_err(err)
    }

    /// The CMS `SignedData` for a document whose signed bytes hash to `digest` (SHA-256).
    fn cms(&self, digest: &[u8], signing_time: u64) -> Result<Vec<u8>> {
        let cert = Cert::parse(&self.certs[0]).ok_or("The certificate could not be read")?;
        let attribute = |o: &str, value: &[u8]| seq(&[&oid(o), &set(&[value])]);
        // DER wants the members of a SET OF in order of their encodings.
        let mut attributes = [
            attribute(OID_CONTENT_TYPE, &oid(OID_DATA)),
            attribute(OID_SIGNING_TIME, &der_time(signing_time)),
            attribute(OID_MESSAGE_DIGEST, &tlv(0x04, digest)),
        ];
        attributes.sort();
        let attributes = attributes.concat();
        // The signature covers the attributes as a SET; inside SignerInfo they are tagged [0].
        let signature = self.key.pkcs1v15_sign::<Sha256>(&tlv(0x31, &attributes)).map_err(err)?;

        let sha256 = seq(&[&oid(OID_SHA256), &NULL]);
        let signer_info = seq(&[
            &asn1::uint(&[1]),
            &seq(&[cert.issuer, cert.serial]),
            &sha256,
            &tlv(0xa0, &attributes),
            &seq(&[&oid(OID_RSA), &NULL]),
            &tlv(0x04, &signature),
        ]);
        let signed_data = seq(&[
            &asn1::uint(&[1]),
            &set(&[&sha256]),
            &seq(&[&oid(OID_DATA)]), // detached: the content is the PDF itself
            &tlv(0xa0, &self.certs.concat()),
            &set(&[&signer_info]),
        ]);
        Ok(seq(&[&oid(OID_SIGNED_DATA), &tlv(0xa0, &signed_data)]))
    }
}

// ---------------------------------------------------------------------- signing

#[derive(Clone, Debug, Default)]
pub struct SignOptions {
    pub reason: String,
    pub location: String,
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Fill the prepared signature slot of a saved PDF. `bytes` is the whole file.
fn fill_slot(bytes: &mut [u8], identity: &Identity, signing_time: u64) -> Result<()> {
    let failed = "The signature could not be prepared in this document";
    // The slot: `<` + zeros + `>`.
    let mut slot = Vec::with_capacity(SLOT_BYTES * 2 + 2);
    slot.push(b'<');
    slot.resize(SLOT_BYTES * 2 + 1, b'0');
    slot.push(b'>');
    let start = find_last(bytes, &slot).ok_or(failed)?;
    let end = start + slot.len();
    // The byte range placeholder that belongs to it (written just before or after).
    let range_at = find_last(bytes, RANGE_PLACEHOLDER.as_bytes()).ok_or(failed)?;
    let range = format!("[0 {start} {end} {}]", bytes.len() - end);
    if range.len() > RANGE_PLACEHOLDER.len() {
        return Err(failed.into());
    }
    let padded = format!("{range:<width$}", width = RANGE_PLACEHOLDER.len());
    bytes[range_at..range_at + padded.len()].copy_from_slice(padded.as_bytes());

    let mut hasher = Sha256::new();
    hasher.update(&bytes[..start]);
    hasher.update(&bytes[end..]);
    let cms = identity.cms(&hasher.finalize(), signing_time)?;
    if cms.len() > SLOT_BYTES {
        return Err("This certificate chain is too large to fit in the signature".into());
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, b) in cms.iter().enumerate() {
        bytes[start + 1 + i * 2] = HEX[(b >> 4) as usize];
        bytes[start + 2 + i * 2] = HEX[(b & 15) as usize];
    }
    Ok(())
}

impl Engine {
    /// Sign the document with `identity` and write it to `target` (`None`: over the opened
    /// file). Unsaved changes are included. Earlier signatures are kept valid when the file
    /// can be updated incrementally.
    pub fn sign_document(&mut self, target: Option<&Path>, identity: &Identity, options: &SignOptions) -> Result<PathBuf> {
        let target = target.unwrap_or(self.path()).to_path_buf();
        let signing_time = now();
        let name = identity.name();
        if self.page_count() == 0 {
            return Err("There is no page to attach the signature to".into());
        }
        let original = self.path().to_path_buf();
        let field_count = self.signatures().len();
        let doc = self.pdf_mut().map_err(|_| "Only PDF files can be signed".to_owned())?;

        // 1. The signature field (an invisible widget on the first page) and its dictionary.
        let mut page = doc.load_pdf_page(0).map_err(err)?;
        let widget = page.add_signature_widget(&format!("Signature{}", field_count + 1)).map_err(err)?;
        let mut field = widget.annotation().object();
        let mut rect = doc.new_array().map_err(err)?;
        for _ in 0..4 {
            rect.array_push(doc.new_int(0).map_err(err)?).map_err(err)?;
        }
        field.dict_put("Rect", rect).map_err(err)?;
        field.dict_put("F", doc.new_int(132).map_err(err)?).map_err(err)?; // print + locked

        let (y, m, d, h, mi, s) = civil(signing_time);
        let mut sig = doc
            .new_object_from_str(&format!(
                "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached \
                 /ByteRange {RANGE_PLACEHOLDER} /Contents <{}> \
                 /M (D:{y:04}{m:02}{d:02}{h:02}{mi:02}{s:02}Z) >>",
                "0".repeat(SLOT_BYTES * 2)
            ))
            .map_err(err)?;
        sig.dict_put("Name", doc.new_string(&name).map_err(err)?).map_err(err)?;
        if !options.reason.trim().is_empty() {
            sig.dict_put("Reason", doc.new_string(options.reason.trim()).map_err(err)?).map_err(err)?;
        }
        if !options.location.trim().is_empty() {
            sig.dict_put("Location", doc.new_string(options.location.trim()).map_err(err)?).map_err(err)?;
        }
        let sig_ref = doc.add_object(&sig).map_err(err)?;
        field.dict_put("V", sig_ref).map_err(err)?;
        drop((widget, page));

        // 2. Save next to the target: incrementally if possible, so earlier signatures survive.
        let mut tmp = target.clone().into_os_string();
        tmp.push(".rr-signing");
        let tmp = PathBuf::from(tmp);
        let tmp_str = tmp.to_str().ok_or("File path is not valid Unicode")?;
        let mut write = PdfWriteOptions::default();
        let incremental = doc.can_be_saved_incrementally();
        let saved = if incremental {
            write.set_incremental(true);
            std::fs::copy(&original, &tmp).map_err(err).and_then(|_| doc.save_with_options(tmp_str, write).map_err(err))
        } else {
            doc.save_with_options(tmp_str, write).map_err(err)
        };

        // 3. Sign the saved bytes.
        let signed = saved.and_then(|()| {
            let mut bytes = std::fs::read(&tmp).map_err(err)?;
            fill_slot(&mut bytes, identity, signing_time)?;
            std::fs::write(&tmp, bytes).map_err(err)
        });
        if let Err(e) = signed {
            let _ = std::fs::remove_file(&tmp);
            // The half-made field must not linger in memory: go back to the file on disk.
            if let Ok(clean) = Engine::open(&original) {
                *self = clean;
            }
            return Err(e);
        }

        // 4. Swap it in and continue from the signed file.
        self.release_file();
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            if let Ok(clean) = Engine::open(&original) {
                *self = clean;
            }
            return Err(format!("Could not write {}: {e}", target.display()));
        }
        *self = Engine::open(&target)?;
        Ok(target)
    }
}

// ----------------------------------------------------------------- verification

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureStatus {
    /// The signed bytes are unchanged and the signature matches the certificate.
    Intact,
    /// The document's signed part was changed, or the signature does not match.
    Broken(String),
    /// A kind of signature this version cannot check.
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct SignatureInfo {
    pub field: String,
    /// Name on the signing certificate (or the name stored in the PDF if it cannot be read).
    pub signer: String,
    pub reason: String,
    pub location: String,
    /// Signing time claimed by the signer's computer (Unix time), if present.
    pub signed_at: Option<u64>,
    pub status: SignatureStatus,
    /// The signature covers the whole file. If false, content was added after signing
    /// (for example comments or another signature); what was signed is still unchanged.
    pub covers_whole_file: bool,
    /// The certificate is its own issuer.
    pub self_signed: bool,
    /// The certificate was within its validity period at the signing time.
    pub certificate_valid_at_signing: bool,
}

/// Check one CMS signature against the digest of the signed bytes.
struct Checked {
    signer: String,
    signed_at: Option<u64>,
    self_signed: bool,
    valid_at_signing: bool,
}

fn check_cms(cms: &[u8], signed: &[&[u8]]) -> std::result::Result<Checked, SignatureStatus> {
    let unreadable = || SignatureStatus::Broken("The signature data is damaged".into());
    let unsupported = |what: &str| SignatureStatus::Unsupported(what.to_owned());

    let (content_info, _) = asn1::read(cms).ok_or_else(unreadable)?;
    let parts = asn1::children(content_info.body).ok_or_else(unreadable)?;
    if parts.first().map(|p| p.raw) != Some(oid(OID_SIGNED_DATA).as_slice()) {
        return Err(unreadable());
    }
    let (signed_data, _) = asn1::read(parts.get(1).ok_or_else(unreadable)?.body).ok_or_else(unreadable)?;
    let fields = asn1::children(signed_data.body).ok_or_else(unreadable)?;
    let certs: Vec<Cert> = fields
        .iter()
        .find(|f| f.tag == 0xa0)
        .and_then(|c| asn1::children(c.body))
        .unwrap_or_default()
        .iter()
        .filter_map(|c| Cert::parse(c.raw))
        .collect();
    let signer_infos = fields.iter().rev().find(|f| f.tag == 0x31).ok_or_else(unreadable)?;
    let signer_info = asn1::children(signer_infos.body).and_then(|s| s.first().copied()).ok_or_else(unreadable)?;
    let si = asn1::children(signer_info.body).ok_or_else(unreadable)?;
    // version, sid, digestAlgorithm, [0] signedAttrs?, signatureAlgorithm, signature, ...
    let sid = si.get(1).ok_or_else(unreadable)?;
    let digest_alg = asn1::children(si.get(2).ok_or_else(unreadable)?.body).ok_or_else(unreadable)?;
    let digest_oid = digest_alg.first().ok_or_else(unreadable)?.raw;
    let (attrs, rest) = match si.get(3) {
        Some(a) if a.tag == 0xa0 => (Some(*a), &si[4..]),
        _ => (None, &si[3..]),
    };
    let signature = rest.get(1).filter(|s| s.tag == 0x04).ok_or_else(unreadable)?.body;

    // Which certificate signed: matched by issuer and serial number.
    let cert = certs
        .iter()
        .find(|c| sid.tag == 0x30 && sid.body == [c.issuer, c.serial].concat())
        .or(certs.first())
        .ok_or_else(|| unsupported("The signature does not include the signer's certificate"))?;
    let key = PublicKey::from_public_key_der(cert.spki)
        .map_err(|_| unsupported("Only RSA signatures can be checked in this version"))?;

    // Hash of the signed bytes, with the algorithm the signer used.
    fn hash<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
        let mut h = D::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().to_vec()
    }
    enum Alg {
        Sha1,
        Sha256,
        Sha384,
        Sha512,
    }
    let alg = [(OID_SHA1, Alg::Sha1), (OID_SHA256, Alg::Sha256), (OID_SHA384, Alg::Sha384), (OID_SHA512, Alg::Sha512)]
        .into_iter()
        .find(|(o, _)| oid(o).as_slice() == digest_oid)
        .map(|(_, a)| a)
        .ok_or_else(|| unsupported("The signature uses a hash algorithm this version cannot check"))?;
    let document_digest = match alg {
        Alg::Sha1 => hash::<Sha1>(signed),
        Alg::Sha256 => hash::<Sha256>(signed),
        Alg::Sha384 => hash::<Sha384>(signed),
        Alg::Sha512 => hash::<Sha512>(signed),
    };
    let verify = |message: &[u8]| match alg {
        Alg::Sha1 => key.pkcs1v15_verify::<Sha1>(message, signature),
        Alg::Sha256 => key.pkcs1v15_verify::<Sha256>(message, signature),
        Alg::Sha384 => key.pkcs1v15_verify::<Sha384>(message, signature),
        Alg::Sha512 => key.pkcs1v15_verify::<Sha512>(message, signature),
    };
    let verify_prehashed = |digest: &[u8]| match alg {
        Alg::Sha1 => key.pkcs1v15_verify_prehash::<Sha1>(digest, signature),
        Alg::Sha256 => key.pkcs1v15_verify_prehash::<Sha256>(digest, signature),
        Alg::Sha384 => key.pkcs1v15_verify_prehash::<Sha384>(digest, signature),
        Alg::Sha512 => key.pkcs1v15_verify_prehash::<Sha512>(digest, signature),
    };

    let mut signed_at = None;
    match attrs {
        Some(attrs) => {
            // The attributes must carry the document's digest, and the signature must match them.
            let mut digest_ok = false;
            for attribute in asn1::children(attrs.body).ok_or_else(unreadable)? {
                let parts = asn1::children(attribute.body).ok_or_else(unreadable)?;
                let (Some(kind), Some(values)) = (parts.first(), parts.get(1)) else { continue };
                let value = asn1::children(values.body).and_then(|v| v.first().copied());
                if kind.raw == oid(OID_MESSAGE_DIGEST).as_slice() {
                    digest_ok = value.is_some_and(|v| v.body == document_digest.as_slice());
                } else if kind.raw == oid(OID_SIGNING_TIME).as_slice() {
                    signed_at = value.and_then(parse_der_time);
                }
            }
            if !digest_ok {
                return Err(SignatureStatus::Broken("The document was changed after it was signed".into()));
            }
            if verify(&tlv(0x31, attrs.body)).is_err() {
                return Err(SignatureStatus::Broken("The signature does not match the signer's certificate".into()));
            }
        }
        None => {
            if verify_prehashed(&document_digest).is_err() {
                return Err(SignatureStatus::Broken("The document was changed after it was signed".into()));
            }
        }
    }

    let at = signed_at.unwrap_or_else(now);
    Ok(Checked {
        signer: cert.display_name(),
        signed_at,
        self_signed: cert.issuer == cert.subject,
        valid_at_signing: cert.not_before <= at && at <= cert.not_after,
    })
}

/// Parse a PDF date such as `D:20261114093000Z` or `D:20261114093000+01'00'` to Unix time.
fn parse_pdf_date(text: &str) -> Option<u64> {
    let s = text.strip_prefix("D:").unwrap_or(text);
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u64>().ok();
    let base = from_civil(n(0..4)? as i64, n(4..6).unwrap_or(1) as i64, n(6..8).unwrap_or(1) as i64, n(8..10).unwrap_or(0), n(10..12).unwrap_or(0), n(12..14).unwrap_or(0));
    let offset = match s.as_bytes().get(14) {
        Some(sign @ (b'+' | b'-')) => {
            let secs = (n(15..17).unwrap_or(0) * 3600 + n(18..20).unwrap_or(0) * 60) as i64;
            if *sign == b'+' { -secs } else { secs }
        }
        _ => 0,
    };
    Some((base as i64 + offset).max(0) as u64)
}

impl Engine {
    /// Every signed signature field in the document, checked against the file on disk.
    pub fn signatures(&self) -> Vec<SignatureInfo> {
        let Some(doc) = self.pdf() else { return Vec::new() };
        let mut fields = Vec::new();
        if let Ok(Some(form)) = doc.catalog().and_then(|c| c.get_dict("AcroForm")) {
            if let Ok(Some(list)) = form.get_dict("Fields") {
                collect_signature_fields(&list, 0, &mut fields);
            }
        }
        if fields.is_empty() {
            return Vec::new();
        }
        let file = std::fs::read(self.path()).unwrap_or_default();
        fields.into_iter().map(|(name, value)| describe_signature(name, &value, &file)).collect()
    }
}

/// Walk the form's field tree for signature fields that have a value.
fn collect_signature_fields(list: &PdfObject, depth: usize, out: &mut Vec<(String, PdfObject)>) {
    if depth > 16 {
        return;
    }
    for i in 0..list.len().unwrap_or(0) {
        let Ok(Some(field)) = list.get_array(i as i32) else { continue };
        if let Ok(Some(kids)) = field.get_dict("Kids") {
            collect_signature_fields(&kids, depth + 1, out);
        }
        let is_signature = matches!(field.get_dict_inheritable("FT"), Ok(Some(t)) if t.as_name().is_ok_and(|n| n == b"Sig"));
        if let (true, Ok(Some(value))) = (is_signature, field.get_dict("V")) {
            if value.get_dict("ByteRange").is_ok_and(|b| b.is_some()) {
                let name = field.get_dict("T").ok().flatten().and_then(|t| t.as_string().ok()).unwrap_or_default();
                out.push((name, value));
            }
        }
    }
}

fn describe_signature(field: String, value: &PdfObject, file: &[u8]) -> SignatureInfo {
    let text = |key: &str| value.get_dict(key).ok().flatten().and_then(|v| v.as_string().ok()).unwrap_or_default();
    let mut info = SignatureInfo {
        field,
        signer: text("Name"),
        reason: text("Reason"),
        location: text("Location"),
        signed_at: parse_pdf_date(&text("M")),
        status: SignatureStatus::Intact,
        covers_whole_file: false,
        self_signed: false,
        certificate_valid_at_signing: false,
    };

    let sub_filter = value.get_dict("SubFilter").ok().flatten().and_then(|s| s.as_name().ok()).unwrap_or_default();
    if !matches!(sub_filter.as_slice(), b"adbe.pkcs7.detached" | b"ETSI.CAdES.detached") {
        info.status = SignatureStatus::Unsupported(format!(
            "This kind of signature ({}) cannot be checked in this version",
            String::from_utf8_lossy(&sub_filter)
        ));
        return info;
    }
    let range: Vec<usize> = value
        .get_dict("ByteRange")
        .ok()
        .flatten()
        .map(|r| {
            (0..r.len().unwrap_or(0))
                .filter_map(|i| r.get_array(i as i32).ok().flatten()?.as_int().ok())
                .map(|v| v.max(0) as usize)
                .collect()
        })
        .unwrap_or_default();
    let cms = value.get_dict("Contents").ok().flatten().and_then(|c| c.as_bytes().ok()).unwrap_or_default();
    let [a0, a1, b0, b1] = range[..] else {
        info.status = SignatureStatus::Broken("The signature's byte range is damaged".into());
        return info;
    };
    if a0 != 0 || a0 + a1 > b0 || b0 + b1 > file.len() {
        info.status = SignatureStatus::Broken("The signature's byte range does not fit this file".into());
        return info;
    }
    info.covers_whole_file = b0 + b1 == file.len();
    match check_cms(&cms, &[&file[a0..a0 + a1], &file[b0..b0 + b1]]) {
        Ok(checked) => {
            info.signer = checked.signer;
            info.signed_at = checked.signed_at.or(info.signed_at);
            info.self_signed = checked.self_signed;
            info.certificate_valid_at_signing = checked.valid_at_signing;
        }
        Err(status) => info.status = status,
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_round_trips() {
        assert_eq!(oid("1.2.840.113549.1.7.2"), [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02]);
        assert_eq!(asn1::uint(&[0x00, 0x80]), [0x02, 0x02, 0x00, 0x80]);
        let long = tlv(0x04, &[7u8; 300]);
        assert_eq!(&long[..4], [0x04, 0x82, 0x01, 0x2c]);
        let (item, rest) = asn1::read(&long).unwrap();
        assert_eq!((item.tag, item.body.len(), rest.len()), (0x04, 300, 0));
        assert!(asn1::read(&long[..100]).is_none(), "truncated input is rejected");
    }

    #[test]
    fn times() {
        let t = 1_794_648_645; // 2026-11-14 09:30:45 UTC
        assert_eq!(civil(t), (2026, 11, 14, 9, 30, 45));
        assert_eq!(from_civil(2026, 11, 14, 9, 30, 45), t);
        assert_eq!(parse_der_time(asn1::read(&der_time(t)).unwrap().0), Some(t));
        assert_eq!(parse_pdf_date("D:20261114093045Z"), Some(t));
        assert_eq!(parse_pdf_date("D:20261114103045+01'00'"), Some(t));
        assert_eq!(format_time(t), "2026-11-14 09:30 UTC");
    }
}
