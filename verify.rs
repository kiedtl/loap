#[cfg(feature = "ssr")]
async fn verify_maintainer(
    conn: &mut sqlx::SqliteConnection,
    maintainer: &str,
    package: &str,
    timestamp: DateTime<Utc>,
    signature: &str,
)
    -> Result<AuthCheck, ServerFnError>
{
    use base64::prelude::*;
    use ring::digest;
    use ring::signature::{UnparsedPublicKey, ED25519};
    use sshkeys::PublicKey;
    use std::io::{BufRead, Read, Cursor};
    use self::ssr::*;

    let data = sqlx::query(
        "SELECT
            m.id, m.pubkey,
            p.id, p.maintainer
        FROM Maintainers m
        INNER JOIN Packages p ON p.name = $2 AND p.maintainer = m.id
        WHERE m.name = $1;"
    )
        .bind(&maintainer)
        .bind(&package)
        .fetch_one(&mut *conn)
        .await;

    let data = match data {
        Ok(d) => d,
        Err(sqlx::Error::RowNotFound) => return Ok(AuthCheck::NotKnown),
        Err(e) => return Err(e.into()),
    };

    let maintainer_id = data.try_get::<i32, _>(0)?;
    let pubkey = data.try_get::<String, _>(1)?;
    let pkg_id = data.try_get::<i32, _>(2)?;
    let pkg_maintainer = data.try_get::<i32, _>(3)?;

    if maintainer_id != pkg_maintainer {
        return Ok(AuthCheck::NotMaintainer);
    }

    let key = PublicKey::from_string(&pubkey)
        .map_err(|e| ServerFnError::new(format!("Couldn't parse public key: {e}")))?;

    let pubkey_bytes = match key.kind {
        sshkeys::PublicKeyKind::Ed25519(key) => key.key,
        _ => return Err(ServerFnError::new("Only ed25519 public keys are supported at this time")),
    };

    let Ok(openssh_signature_bytes) = BASE64_STANDARD.decode(signature) else {
        return Ok(AuthCheck::SignatureMalformed);
    };
    let mut cursor = Cursor::new(&openssh_signature_bytes);

    macro_rules! read_chunk {
        ($cursor:expr) => {{
            let mut len_bytes = [0u8; 4];
            let Ok(_) = $cursor.read_exact(&mut len_bytes) else {
                return Ok(AuthCheck::SignatureMalformed);
            };
            let len = u32::from_be_bytes(len_bytes) as usize;

            let mut data = vec![0u8; len];
            let Ok(_) = $cursor.read_exact(&mut data) else {
                return Ok(AuthCheck::SignatureMalformed);
            };
            data
        }}
    }

    // Extract signature data
    // https://github.com/openssh/openssh-portable/blob/master/PROTOCOL.sshsig

    let mut magic = [0u8; 6];
    let Ok(_) = cursor.read_exact(&mut magic) else {
        return Ok(AuthCheck::SignatureMalformed);
    };

    if &magic != b"SSHSIG" {
        leptos::logging::log!("bad magic: {:?}", magic);
        return Ok(AuthCheck::SignatureMalformed);
    }

    // Ignore SIG_VERSION (TODO: don't?)
    cursor.consume(4);

    let _sig_publickey = read_chunk!(cursor);
        leptos::logging::log!("ok: pkey");
    let namespace = read_chunk!(cursor);
        leptos::logging::log!("ok: namespace {:?}", String::from_utf8(namespace.clone()).unwrap());
    let _reserved = read_chunk!(cursor);
        leptos::logging::log!("ok: reserved");
    let algorithm = read_chunk!(cursor);
        leptos::logging::log!("ok: algorithm {:?}", String::from_utf8(algorithm.clone()).unwrap());
    let signature_chunk = read_chunk!(cursor);

    let mut signature_cursor = Cursor::new(&signature_chunk);
    let _signature_algorithm = read_chunk!(signature_cursor);
        leptos::logging::log!("ok: signature algorithm {:?}", String::from_utf8(_signature_algorithm).unwrap());
    let signature_data = read_chunk!(signature_cursor);
        leptos::logging::log!("ok: signature blob {:?}", signature_data.len());

    if namespace != b"loap-file-upload" {
        leptos::logging::log!("bad namespace");
        return Ok(AuthCheck::SignatureMalformed);
    }

    let verifier = UnparsedPublicKey::new(&ED25519, pubkey_bytes);
    let message = format!("{maintainer}:{package}:{}", timestamp.timestamp());
    let hashed = match &algorithm[0..] {
        b"sha256" => digest::digest(&digest::SHA256, message.as_bytes()),
        b"sha512" => digest::digest(&digest::SHA512, message.as_bytes()),
        _ => {
            leptos::logging::log!("unknown digest");
            return Ok(AuthCheck::SignatureMalformed);
        }
    };

    Ok(match verifier.verify(hashed.as_ref(), &signature_data) {
        Ok(_) => AuthCheck::Ok(maintainer_id, pkg_id),
        Err(_) => AuthCheck::SignatureFailed,
    })
}
