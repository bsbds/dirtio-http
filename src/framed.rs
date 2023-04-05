use crate::codec::{Decoder, Encoder};

use std::io;
use std::task::{ready, Poll};

use bytes::{Buf, BytesMut};
use dirtio::net::tcp::TcpStream;
use futures::{AsyncRead, AsyncWrite, Sink, Stream};
use pin_project_lite::pin_project;

pin_project! {
    pub struct HttpFramed<C> {
        #[pin]
        inner: TcpStream,
        codec: C,
        state: RWFrame,
    }
}

impl<C> HttpFramed<C> {
    pub fn new(inner: TcpStream, codec: C) -> Self {
        Self {
            inner,
            codec,
            state: RWFrame::default(),
        }
    }
}

impl<C> Stream for HttpFramed<C>
where
    C: Decoder,
{
    type Item = Result<C::Item, C::Error>;

    // A state machine for reading and framing.
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let state = &mut this.state.read;

        loop {
            if state.readable {
                if state.eof {
                    // Reach stream EOF but we need to decode
                    // all remaining bytes.
                    match this.codec.decode_eof(&mut state.buffer)? {
                        Some(frame) => {
                            return Poll::Ready(Some(Ok(frame)));
                        }
                        None => {
                            return Poll::Ready(None);
                        }
                    }
                }

                if let Some(frame) = this.codec.decode(&mut state.buffer)? {
                    return Poll::Ready(Some(Ok(frame)));
                }

                state.readable = false;
            }

            // A dummy implementation.
            //
            // Read to a tmp buffer first, and then
            // copy to the main buffer, may be slow
            // to have an extra copy.
            let n = ready!(this.inner.as_mut().poll_read(cx, &mut state.tmp_buf))?;

            if n == 0 {
                state.eof = true;
            } else {
                state.buffer.extend_from_slice(&state.tmp_buf[0..n]);
            }

            state.readable = true;
        }
    }
}

impl<I, C> Sink<I> for HttpFramed<C>
where
    C: Encoder<I>,
    C::Error: From<io::Error>,
{
    type Error = C::Error;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let state = &self.state.write;

        if state.buffer.len() > state.backpressure_boundary {
            self.poll_flush(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: I) -> Result<(), Self::Error> {
        let this = self.project();
        let state = &mut this.state.write;
        this.codec.encode(item, &mut state.buffer)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        let state = &mut this.state.write;

        while !state.buffer.is_empty() {
            let n = ready!(this.inner.as_mut().poll_write(cx, &state.buffer.chunk())?);
            state.buffer.advance(n);

            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write to stream",
                )
                .into()));
            }
        }

        ready!(this.inner.poll_flush(cx))?;

        Poll::Ready(Ok(()))
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        // flush before close
        ready!(self.as_mut().poll_flush(cx))?;

        let this = self.project();
        ready!(this.inner.poll_close(cx))?;

        Poll::Ready(Ok(()))
    }
}

const INITIAL_CAPACITY: usize = 8 * 1024;

#[derive(Default)]
struct RWFrame {
    read: ReadFrame,
    write: WriteFrame,
}

struct ReadFrame {
    eof: bool,
    readable: bool,
    buffer: BytesMut,
    tmp_buf: [u8; INITIAL_CAPACITY],
}

struct WriteFrame {
    backpressure_boundary: usize,
    buffer: BytesMut,
}

impl Default for ReadFrame {
    fn default() -> Self {
        Self {
            eof: false,
            readable: false,
            buffer: BytesMut::with_capacity(INITIAL_CAPACITY),
            tmp_buf: [0u8; INITIAL_CAPACITY],
        }
    }
}

impl Default for WriteFrame {
    fn default() -> Self {
        Self {
            backpressure_boundary: INITIAL_CAPACITY,
            buffer: BytesMut::with_capacity(INITIAL_CAPACITY),
        }
    }
}
