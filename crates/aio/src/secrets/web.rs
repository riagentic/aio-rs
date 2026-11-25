//! The web: each secret sealed (AES-GCM) by a key derived (HKDF) from the
//! output of the app's passkey's PRF (WebAuthn's `prf` extension), kept
//! as ciphertext in the page's `localStorage`, where the app's cells are.
//! The passkey is made on the first `set`; the key exists only while a
//! call runs, after the user verified the passkey, and is never stored:
//! the browser gives the PRF's output to this origin's page alone, and only
//! through the passkey.
//!
//! `localStorage` over IndexedDB: the page keeps its cells there already,
//! one write is whole (no transaction to keep), and both are the origin's
//! alike, readable by any script on it, so only ciphertext is put there.
use base64::Engine;
use serde_json::{Value, json};
use std::io;

/// The envelope's first byte: how it was sealed (this file's scheme).
const V1: u8 = 1;
/// AES-GCM's nonce and tag, in bytes.
const IV: usize = 12;
const TAG: usize = 16;
/// What the passkey's PRF is asked about (the browser mixes in its own
/// context first): one key per passkey.
const PRF_INPUT: &[u8] = b"aio-rs secrets v1";
/// Where the passkey's id is kept.
const PASSKEY: &str = "aio-secrets/passkey";

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Where secret `name` is kept (pure).
fn key(name: &str) -> String {
    format!("aio-secrets/s_{name}")
}

/// HKDF's info for secret `name`: each secret its own key, so one moved
/// under another name opens under none (pure).
fn info(name: &str) -> Vec<u8> {
    [b"aio-rs secret\0".as_slice(), name.as_bytes()].concat()
}

/// The passkey's user handle on the page at `origin`: the same each time,
/// so a passkey made again replaces the old one on its device, and nothing
/// about the user (pure).
fn user_id(origin: &str) -> [u8; 16] {
    let h = crate::sha256::Sha256::digest([b"aio-rs secrets\0".as_slice(), origin.as_bytes()].concat());
    h[..16].try_into().expect("16 of 32")
}

/// What the first `set` asks the browser to make: this origin's passkey for
/// the app's secrets, user-verified, with its PRF. Its binary fields
/// (`challenge`, `user.id`, the PRF's input) are set after (pure).
fn creation(title: &str) -> Value {
    let label = format!("{title}: secrets kept in this browser");
    let algs = [-7, -8, -257].map(|alg| json!({ "type": "public-key", "alg": alg }));
    json!({ "publicKey": {
        "rp": { "name": title },
        "user": { "name": label, "displayName": label },
        "pubKeyCredParams": algs,
        "authenticatorSelection": { "residentKey": "preferred", "userVerification": "required" },
        "extensions": { "prf": { "eval": {} } },
    }})
}

/// What each `set` and `get` asks of the passkey: its PRF, user-verified
/// (hmac-secret's PRF differs with and without it, so always with). The
/// binary fields (`challenge`, the credential's `id`, the PRF's input) are
/// set after (pure).
fn request() -> Value {
    json!({ "publicKey": {
        "allowCredentials": [{ "type": "public-key" }],
        "userVerification": "required",
        "extensions": { "prf": { "eval": {} } },
    }})
}

/// Secret `name`'s envelope: the scheme's byte, AES-GCM's nonce, then the
/// ciphertext with its tag, as base64 (pure).
fn seal(iv: &[u8; IV], ct: &[u8]) -> String {
    B64.encode([&[V1][..], iv, ct].concat())
}

/// The nonce and ciphertext of secret `name`'s envelope `text` (pure).
fn open(name: &str, text: &str) -> io::Result<([u8; IV], Vec<u8>)> {
    let bytes = B64.decode(text).unwrap_or_default();
    match bytes.split_first() {
        Some((&V1, rest)) if rest.len() >= IV + TAG => {
            Ok((rest[..IV].try_into().expect("IV bytes"), rest[IV..].to_vec()))
        }
        Some((&v, _)) if v > V1 => Err(invalid(format!(
            "secret {name} was sealed by a newer aio-rs (scheme {v}), or changed: update the app, or delete it \
             and set it again"
        ))),
        _ => Err(unopened(name)),
    }
}

fn invalid(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

fn unopened(name: &str) -> io::Error {
    invalid(format!(
        "secret {name} can't be opened by this page's passkey (a passkey made since, or a stored copy changed): \
         delete it and set it again"
    ))
}

fn no_prf() -> io::Error {
    super::unsupported(
        "this browser or passkey has no PRF (WebAuthn's key derivation), so nothing was kept: use a passkey \
         provider that has it (Google Password Manager, iCloud Keychain, a security key with hmac-secret) \
         in a current browser, or ask the user each time",
    )
}

/// The error a passkey's or WebCrypto's `DOMException` named `dom` says (pure).
fn refused(dom: &str, msg: &str) -> io::Error {
    match dom {
        "NotAllowedError" => io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the passkey was not used (cancelled, timed out, or not on this device): {msg}"),
        ),
        "SecurityError" | "NotSupportedError" => {
            super::unsupported(format!("this page can't use a passkey ({msg}): ask the user each time"))
        }
        _ => io::Error::other(format!("{dom}: {msg}")),
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) use page::{delete, get, put};

#[cfg(target_arch = "wasm32")]
mod page {
    use super::super::Secret;
    use super::*;
    use crate::web::{Local, storage::Kv};
    use js_sys::{Array, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    /// One call at a time: the browser asks the user one thing at a time.
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    pub(in super::super) async fn put(name: &str, v: &[u8]) -> io::Result<()> {
        super::super::check(name)?;
        let _one = ONE.lock().await;
        let (w, k) = aes(name, true).await?;
        let iv = crate::random_bytes::<IV>();
        let ct = bytes(&wait(subtle(&w)?.encrypt_with_object_and_u8_array(&gcm(&iv), &k, v)).await?);
        Local.set(&key(name), &seal(&iv, &ct)).map_err(|e| io::Error::other(format!("secret {name} not kept: {e}")))
    }

    pub(in super::super) async fn get(name: &str) -> io::Result<Option<Secret>> {
        super::super::check(name)?;
        let _one = ONE.lock().await;
        let Some(text) = Local.get(&key(name)) else { return Ok(None) };
        let (iv, ct) = open(name, &text)?;
        let (w, k) = aes(name, false).await?;
        match wait(subtle(&w)?.decrypt_with_object_and_u8_array(&gcm(&iv), &k, &ct)).await {
            Ok(v) => Ok(Some(Secret(bytes(&v)))),
            Err(_) => Err(unopened(name)),
        }
    }

    pub(in super::super) async fn delete(name: &str) -> io::Result<()> {
        super::super::check(name)?;
        let _one = ONE.lock().await;
        Local.remove(&key(name));
        Ok(())
    }

    /// Secret `name`'s AES-GCM key, from the passkey's PRF (asking the
    /// user); with no passkey yet, one made when `make`.
    async fn aes(name: &str, make: bool) -> io::Result<(web_sys::Window, web_sys::CryptoKey)> {
        let w = web_sys::window().ok_or_else(|| {
            super::super::unsupported("a passkey asks the user in the page: call aio::secrets there, not in a worker")
        })?;
        if !w.is_secure_context() {
            return Err(super::super::unsupported(
                "passkeys need a secure page (https, or http://localhost): ask the user each time",
            ));
        }
        let origin = w.location().origin().map_err(js)?;
        let id = Local.get(PASSKEY).and_then(|t| B64.decode(t).ok());
        let prf = match id {
            Some(id) => prf(&w, &id).await?,
            None if make => made(&w, &origin).await?,
            None => return Err(unopened(name)),
        };
        let s = subtle(&w)?;
        let usage = Array::of1(&"deriveKey".into());
        let raw = Uint8Array::new_from_slice(&prf);
        let base = wait(s.import_key_with_object("raw", &raw, &obj(&[("name", "HKDF".into())]), false, &usage)).await?;
        let hkdf = obj(&[
            ("name", "HKDF".into()),
            ("hash", "SHA-256".into()),
            ("salt", Uint8Array::new_from_slice(origin.as_bytes()).into()),
            ("info", Uint8Array::new_from_slice(&info(name)).into()),
        ]);
        let aes = obj(&[("name", "AES-GCM".into()), ("length", 256.into())]);
        let usages = Array::of2(&"encrypt".into(), &"decrypt".into());
        let k = wait(s.derive_key_with_object_and_object(&hkdf, base.unchecked_ref(), &aes, false, &usages)).await?;
        Ok((w, k.unchecked_into()))
    }

    /// The app's passkey, made (the browser asks the user): its id kept,
    /// its PRF's output. A passkey without PRF is not kept, and the
    /// browser is told to forget it where it can.
    async fn made(w: &web_sys::Window, origin: &str) -> io::Result<Secret> {
        if browser_has_no_prf().await {
            return Err(no_prf());
        }
        let title = w.document().map(|d| d.title()).filter(|t| !t.trim().is_empty());
        let title = title.unwrap_or_else(|| w.location().host().unwrap_or_default());
        let o = options(&creation(&title))?;
        set(&o, &["publicKey", "user", "id"], &user_id(origin));
        let c = wait(w.navigator().credentials().create_with_options(o.unchecked_ref())).await?;
        let id = bytes(&at(&c, &["rawId"]));
        let out = at(&call(&c, "getClientExtensionResults", &JsValue::UNDEFINED)?, &["prf"]);
        let first = at(&out, &["results", "first"]);
        if at(&out, &["enabled"]) != JsValue::TRUE && first.is_undefined() {
            forget(w, &id);
            return Err(no_prf());
        }
        Local
            .set(PASSKEY, &B64.encode(&id))
            .map_err(|e| io::Error::other(format!("the passkey's id not kept: {e}")))?;
        match first.is_undefined() {
            true => prf(w, &id).await,
            false => Ok(Secret(bytes(&first))),
        }
    }

    /// The PRF's output of passkey `id` (the browser asks the user).
    async fn prf(w: &web_sys::Window, id: &[u8]) -> io::Result<Secret> {
        let o = options(&request())?;
        set(&o, &["publicKey", "allowCredentials", "0", "id"], id);
        let a = wait(w.navigator().credentials().get_with_options(o.unchecked_ref())).await?;
        let first = at(&call(&a, "getClientExtensionResults", &JsValue::UNDEFINED)?, &["prf", "results", "first"]);
        match first.is_undefined() {
            true => Err(no_prf()),
            false => Ok(Secret(bytes(&first))),
        }
    }

    /// Does the browser say it has no PRF (`getClientCapabilities`, where
    /// it has that)? Then no passkey is made in vain.
    async fn browser_has_no_prf() -> bool {
        let pkc = Reflect::get(&js_sys::global(), &"PublicKeyCredential".into()).unwrap_or_default();
        let Ok(caps) = call(&pkc, "getClientCapabilities", &JsValue::UNDEFINED) else { return false };
        let caps = JsFuture::from(js_sys::Promise::resolve(&caps)).await.unwrap_or_default();
        at(&caps, &["extension:prf"]) == JsValue::FALSE
    }

    /// Tell the browser's passkey provider passkey `id` is of no use
    /// (`signalUnknownCredential`, where it has that).
    fn forget(w: &web_sys::Window, id: &[u8]) {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let pkc = Reflect::get(&js_sys::global(), &"PublicKeyCredential".into()).unwrap_or_default();
        let rp = w.location().hostname().unwrap_or_default();
        let it = obj(&[("rpId", rp.into()), ("credentialId", URL_SAFE_NO_PAD.encode(id).into())]);
        _ = call(&pkc, "signalUnknownCredential", &it);
    }

    /// The options `o` in the page, with a fresh challenge and the PRF's
    /// input (the passkey signs nothing anyone checks: only its PRF is used).
    fn options(o: &Value) -> io::Result<JsValue> {
        let o = js_sys::JSON::parse(&o.to_string()).map_err(js)?;
        set(&o, &["publicKey", "challenge"], &crate::random_bytes::<32>());
        set(&o, &["publicKey", "extensions", "prf", "eval", "first"], PRF_INPUT);
        Ok(o)
    }

    fn subtle(w: &web_sys::Window) -> io::Result<web_sys::SubtleCrypto> {
        Ok(w.crypto().map_err(js)?.subtle())
    }

    fn gcm(iv: &[u8; IV]) -> js_sys::Object {
        obj(&[("name", "AES-GCM".into()), ("iv", Uint8Array::new_from_slice(iv).into())])
    }

    fn obj(fields: &[(&str, JsValue)]) -> js_sys::Object {
        let o = js_sys::Object::new();
        for (k, v) in fields {
            _ = Reflect::set(&o, &(*k).into(), v);
        }
        o
    }

    /// `v`'s field at `path` (`undefined` where there is none).
    fn at(v: &JsValue, path: &[&str]) -> JsValue {
        path.iter().fold(v.clone(), |v, k| match v.is_object() {
            true => Reflect::get(&v, &(*k).into()).unwrap_or_default(),
            false => JsValue::UNDEFINED,
        })
    }

    /// Field `path` of `o`, which has its parents, made the bytes `b`.
    fn set(o: &JsValue, path: &[&str], b: &[u8]) {
        let (last, parents) = path.split_last().expect("a path");
        _ = Reflect::set(&at(o, parents), &(*last).into(), &Uint8Array::new_from_slice(b));
    }

    /// `this.<method>(arg)`.
    fn call(this: &JsValue, method: &str, arg: &JsValue) -> io::Result<JsValue> {
        let f: js_sys::Function =
            at(this, &[method]).dyn_into().map_err(|_| io::Error::other(format!("no {method}")))?;
        f.call1(this, arg).map_err(js)
    }

    /// The bytes of an `ArrayBuffer` or a view of one.
    fn bytes(v: &JsValue) -> Vec<u8> {
        match v.dyn_ref::<js_sys::ArrayBuffer>() {
            Some(b) => Uint8Array::new(b).to_vec(),
            None => v.dyn_ref::<Uint8Array>().map(Uint8Array::to_vec).unwrap_or_default(),
        }
    }

    async fn wait(p: Result<js_sys::Promise, JsValue>) -> io::Result<JsValue> {
        JsFuture::from(p.map_err(js)?).await.map_err(js)
    }

    /// A thrown JS value as an error, by its `DOMException`'s name.
    fn js(e: JsValue) -> io::Error {
        let s = |k: &str| at(&e, &[k]).as_string().unwrap_or_default();
        refused(&s("name"), &s("message"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_is_the_scheme_the_nonce_and_the_ciphertext() {
        let iv = [7u8; IV];
        let ct = [9u8; TAG + 3];
        let text = seal(&iv, &ct);
        assert_eq!(B64.decode(&text).unwrap(), [&[1u8][..], &iv, &ct].concat(), "v1: 1, nonce, ciphertext and tag");
        assert_eq!(open("a", &text).unwrap(), (iv, ct.to_vec()));
        // An empty secret is its tag alone.
        assert_eq!(open("a", &seal(&iv, &[0; TAG])).unwrap().1.len(), TAG);
    }

    #[test]
    fn a_broken_or_newer_envelope_is_refused_saying_what_to_do() {
        let short = B64.encode([&[1u8][..], &[0; IV + TAG - 1]].concat());
        let zero = B64.encode([0u8; 40]);
        for bad in ["", "not base64!", &short, &zero] {
            let e = open("tok", bad).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{bad:?}");
            assert!(e.to_string().contains("secret tok") && e.to_string().contains("set it again"), "{e}");
        }
        let e = open("tok", &B64.encode([2u8; 40])).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("newer aio-rs (scheme 2), or changed: update the app"), "{e}");
    }

    #[test]
    fn each_secret_has_its_own_key_and_place() {
        assert_eq!(info("session"), b"aio-rs secret\0session");
        assert_ne!(info("a"), info("b"));
        assert_eq!(key("session"), "aio-secrets/s_session");
        assert_ne!(key("passkey"), PASSKEY, "no name lands on the passkey's id");
        assert!(!key("x").starts_with(&crate::web::storage::prefix("x")), "apart from the cells' documents");
        assert_eq!(PRF_INPUT, b"aio-rs secrets v1", "changing it loses every kept secret: a new scheme");
    }

    #[test]
    fn an_origin_has_one_user_handle_naming_no_one() {
        let a = user_id("https://a.example");
        assert_eq!(a, user_id("https://a.example"), "made again: the same, so it replaces the old passkey");
        assert_ne!(a, user_id("https://b.example"));
    }

    #[test]
    fn the_passkey_is_user_verified_and_asked_for_its_prf() {
        let c = creation("Notes");
        let p = &c["publicKey"];
        assert_eq!(p["rp"], json!({ "name": "Notes" }));
        assert_eq!(p["user"]["name"], "Notes: secrets kept in this browser");
        assert_eq!(p["authenticatorSelection"]["userVerification"], "required");
        assert_eq!(p["extensions"], json!({ "prf": { "eval": {} } }));
        assert_eq!(p["pubKeyCredParams"][0], json!({ "type": "public-key", "alg": -7 }));
        let r = request();
        let p = &r["publicKey"];
        assert_eq!(p["userVerification"], "required");
        assert_eq!(p["allowCredentials"], json!([{ "type": "public-key" }]), "this app's passkey only");
        assert_eq!(p["extensions"], json!({ "prf": { "eval": {} } }));
    }

    #[test]
    fn a_passkeys_refusal_says_its_kind() {
        let kind = |dom| refused(dom, "m").kind();
        assert_eq!(kind("NotAllowedError"), io::ErrorKind::PermissionDenied);
        assert_eq!(kind("SecurityError"), io::ErrorKind::Unsupported);
        assert_eq!(kind("NotSupportedError"), io::ErrorKind::Unsupported);
        assert_eq!(kind("OperationError"), io::ErrorKind::Other);
        assert!(refused("SecurityError", "m").to_string().contains("ask the user each time"));
        let e = no_prf();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("no PRF") && e.to_string().contains("ask the user each time"), "{e}");
    }
}
