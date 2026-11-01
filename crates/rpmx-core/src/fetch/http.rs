use std::io::Read;

use crate::error::{codes, Error, Result};

pub(crate) fn proxy_get(agent: &ureq::Agent, proxy: &str, hex: &str) -> Result<Option<Vec<u8>>> {
    let url = format!("{}/v1/blobs/sha512/{hex}", proxy.trim_end_matches('/'));
    match agent.get(&url).call() {
        Ok(mut response) => Ok(Some(read_body(response.body_mut())?)),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(error) => Err(Error::new(
            codes::FETCH_003,
            format!("proxy request failed: {error}"),
        )),
    }
}

pub(crate) fn read_body(body: &mut ureq::Body) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    body.as_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot read response: {error}")))?;
    Ok(bytes)
}
