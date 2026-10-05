//! A scripted [`Http`] for the unit tests: answers come off a queue and every request is kept.

use porter_http::{Http, HttpError, HttpRequest, HttpResponse, Status};
use std::collections::VecDeque;
use std::sync::Mutex;

pub(crate) struct Scripted {
    answers: Mutex<VecDeque<Result<HttpResponse, HttpError>>>,
    pub(crate) seen: Mutex<Vec<HttpRequest>>,
}

impl Scripted {
    pub(crate) fn new(answers: Vec<Result<HttpResponse, HttpError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn body_of(&self, at: usize) -> String {
        String::from_utf8_lossy(&self.seen.lock().expect("lock")[at].body).into_owned()
    }
}

pub(crate) fn answer(status: u16, body: &str) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status: Status(status),
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    })
}

impl Http for Scripted {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.seen.lock().expect("lock").push(request);
        self.answers
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(Err(HttpError::Unreachable))
    }
}
