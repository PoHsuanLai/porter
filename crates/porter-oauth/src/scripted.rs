//! A scripted [`Http`] for the unit tests: answers come off a queue and every request is kept.

use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use std::collections::VecDeque;
use std::sync::Mutex;

pub(crate) struct Scripted {
    answers: Mutex<VecDeque<Result<Response<Vec<u8>>, HttpError>>>,
    pub(crate) seen: Mutex<Vec<Request<Vec<u8>>>>,
}

impl Scripted {
    pub(crate) fn new(answers: Vec<Result<Response<Vec<u8>>, HttpError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn body_of(&self, at: usize) -> String {
        String::from_utf8_lossy(self.seen.lock().expect("lock")[at].body()).into_owned()
    }
}

pub(crate) fn answer(status: u16, body: &str) -> Result<Response<Vec<u8>>, HttpError> {
    Ok(Response::builder()
        .status(status)
        .body(body.as_bytes().to_vec())
        .expect("response"))
}

impl Http for Scripted {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        self.seen.lock().expect("lock").push(request);
        self.answers
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Err(HttpError::Unreachable))
    }
}
