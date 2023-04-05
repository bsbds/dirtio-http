use std::io;

use bytes::BytesMut;
use http::{Request, Response, Version};

pub trait Encoder<Item> {
    type Error: From<io::Error>;

    fn encode(&mut self, item: Item, dst: &mut BytesMut) -> Result<(), Self::Error>;
}

pub trait Decoder {
    type Item;

    type Error: From<io::Error>;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error>;

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(frame) => Ok(Some(frame)),
            None => {
                if src.is_empty() {
                    return Ok(None);
                } else {
                    return Err(io::Error::new(io::ErrorKind::Other, "remaining bytes").into());
                }
            }
        }
    }
}

pub struct HttpCodec;

impl Encoder<Response<Vec<u8>>> for HttpCodec {
    type Error = io::Error;

    fn encode(
        &mut self,
        response: Response<Vec<u8>>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let status = response.status();

        let status_line = format!(
            "\
            HTTP/1.1 {} {}\r\n\
            Server: dirtio-http\r\n\
            Date: {}\r\n\
            Content-Length: {}\r\n\
            ",
            status.as_str(),
            status.canonical_reason().unwrap(),
            httpdate::fmt_http_date(std::time::SystemTime::now()),
            response.body().len(),
        );
        dst.extend_from_slice(status_line.as_bytes());

        for (key, value) in response.headers() {
            let header = format!(
                "{}: {}\r\n",
                key.as_str(),
                value.to_str().unwrap_or_else(|_| "")
            );
            dst.extend_from_slice(header.as_bytes());
        }
        // end of headers
        dst.extend_from_slice(b"\r\n");

        dst.extend_from_slice(&response.into_body());

        Ok(())
    }
}

impl Decoder for HttpCodec {
    type Item = Request<()>;

    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let mut request = Request::builder();
        let mut parsed_headers = [httparse::EMPTY_HEADER; 64];
        let mut r = httparse::Request::new(&mut parsed_headers);
        let status = r
            .parse(src)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let offset = match status {
            httparse::Status::Complete(offset) => offset,
            httparse::Status::Partial => return Ok(None),
        };

        // Only support HTTP 1.1
        if r.version != Some(1) {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "version not supported",
            ));
        }

        request = request.method(r.method.unwrap());
        request = request.uri(r.path.unwrap());
        request = request.version(Version::HTTP_11);

        for header in r.headers.iter() {
            request = request.header(header.name, header.value);
        }

        let _ = src.split_to(offset);

        Ok(Some(
            request
                .body(())
                .map_err(|op| io::Error::new(io::ErrorKind::Other, op))?,
        ))
    }
}
