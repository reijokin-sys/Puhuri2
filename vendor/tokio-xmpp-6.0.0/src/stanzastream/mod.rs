// Modified for Puhuri 2, 2026-10-24: durable ingress and split receiver unlock wakes.
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2019 Emmanuel Gil Peyrot <linkmauve@linkmauve.fr>
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! # Resilient stanza stream
//!
//! This module provides the [`StanzaStream`], which is the next level up from
//! the low-level [`XmlStream`][`crate::xmlstream::XmlStream`].
//!
//! The stanza stream knows about XMPP and it most importantly knows how to
//! fix a broken connection with a reconnect and how to do this smoothly using
//! [XEP-0198 (Stream Management)](https://xmpp.org/extensions/xep-0198.html).
//! XEP-0198 is only used if the peer supports it. If the peer does not
//! support XEP-0198, automatic reconnects are still done, but with more
//! undetectable data loss.
//!
//! The main API entrypoint for the stanza stream is, unsurprisingly,
//! [`StanzaStream`].

use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

// TODO: ensure that IDs are always set on stanzas.

// TODO: figure out what to do with the mpsc::Sender<QueueEntry> on lossy
// stream reconnects. Keeping it may cause stanzas to be sent which weren't
// meant for that stream, replacing it is racy.

use futures::{SinkExt, Stream};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::{mpsc, oneshot};
use xmpp_parsers::{jid::Jid, stream_features::StreamFeatures};

use crate::connect::ServerConnector;
use crate::xmlstream::Timeouts;
use crate::Stanza;

mod connected;
mod error;
mod negotiation;
mod queue;
mod stream_management;
#[cfg(test)]
pub(crate) mod tests;
mod worker;

use self::queue::QueueEntry;
pub use self::queue::{StanzaStage, StanzaState, StanzaToken};
pub use self::stream_management::SmSnapshot;
pub use self::worker::{Connection, XmppStream};
use self::worker::{StanzaStreamWorker, LOCAL_SHUTDOWN_TIMEOUT};

/// Synchronous durable ingress barrier, run on the sole transport worker.
///
/// Success must mean the stanza is durable or has a final policy disposition.
/// The worker does not drive transport writes (including SM acks) while this
/// callback runs. Failure is terminal: drop transport without a stream footer
/// or reconnect. Do not include stanza content/credentials in error text.
/// This does not itself provide snapshot/resume or durable outbound replay.
/// Only parsed application stanzas reach the barrier: library-handled bind
/// replies and malformed stanza parse errors do not. Pre-SM stanzas can reach
/// it, so callback count must not be used as a protocol SM counter.
pub type InboundCommit = Arc<dyn Fn(&Stanza) -> std::io::Result<()> + Send + Sync>;

/// State at an inbound durability barrier, captured by the sole transport worker.
/// `ready=false` means a pre-SM/bind negotiation stanza: persist the stanza,
/// but do not replace a prior stream checkpoint with this provisional state.
pub struct IngressContext {
    /// Authenticated identity (bound identity once ready).
    pub jid: Jid,
    /// True only once the stream negotiation has completed.
    pub ready: bool,
    /// Actual protocol counters plus outbound state at this event boundary.
    pub state: DetachedState,
}

/// Context-aware ingress. `None` is a malformed stanza the library has finally
/// discarded and counted; checkpoint that disposition without storing XML.
/// Success must make the incoming stanza/checkpoint durable before any ack.
pub type CheckpointCommit =
    Arc<dyn Fn(Option<&Stanza>, IngressContext) -> std::io::Result<()> + Send + Sync>;

fn copy_stanza(stanza: &Stanza) -> Stanza {
    match stanza {
        Stanza::Iq(v) => v.clone().into(),
        Stanza::Message(v) => v.clone().into(),
        Stanza::Presence(v) => v.clone().into(),
    }
}

/// Event informing about the change of the [`StanzaStream`]'s status.
#[derive(Debug)]
pub enum StreamEvent {
    /// The stream was (re-)established **with** loss of state.
    Reset {
        /// The new JID to which the stream is bound.
        bound_jid: Jid,

        /// The features reported by the stream.
        features: StreamFeatures,
    },

    /// The stream is currently inactive because a connection was lost.
    ///
    /// Resumption without loss of state is still possible. This event is
    /// merely informative and may be used to prolong timeouts or inform the
    /// user that the connection is currently unstable.
    Suspended,

    /// The stream was reestablished **without** loss of state.
    ///
    /// This is merely informative. Potentially useful to prolong timeouts.
    Resumed {
        /// The identity retained by the resumed session.
        bound_jid: Jid,
        /// Features of the new transport.
        features: StreamFeatures,
    },
}

/// Event emitted by the [`StanzaStream`].
///
/// Note that stream closure is not an explicit event, but the end of the
/// event stream itself.
#[derive(Debug)]
pub enum Event {
    /// The streams connectivity status has changed.
    Stream(StreamEvent),

    /// A stanza was received over the stream.
    Stanza(Stanza),
}

/// Complete owned data returned by a quiet transport detach.
/// Persist both SM and unsent work before another process takes ownership.
#[derive(PartialEq)]
pub struct DetachedState {
    /// Negotiated SM state; its token may be absent when resumption was disallowed.
    pub sm: Option<SmSnapshot>,
    /// Not-yet-sent work, separate from the SM unacknowledged wire sequence.
    pub pending: Vec<Stanza>,
}

impl std::fmt::Debug for DetachedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetachedState")
            .field("sm", &self.sm)
            .field("pending_count", &self.pending.len())
            .finish()
    }
}

#[derive(Debug, Default)]
struct ConnectorTasks(std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>);
impl ConnectorTasks {
    fn take(&self) -> Vec<tokio::task::JoinHandle<()>> {
        std::mem::take(&mut *self.0.lock().expect("connector task lock poisoned"))
    }
}
impl Drop for ConnectorTasks {
    fn drop(&mut self) {
        for task in self.take() {
            task.abort();
        }
    }
}

/// Frontend interface to a reliable, always-online stanza stream.
#[derive(Debug)]
pub struct StanzaStream {
    rx: mpsc::Receiver<Event>,
    tx: mpsc::Sender<QueueEntry>,
    detach: Option<oneshot::Sender<()>>,
    worker: tokio::task::JoinHandle<Option<DetachedState>>,
    durable_ingress: bool,
    connector_tasks: Option<Arc<ConnectorTasks>>,
}

impl StanzaStream {
    /// Establish a new client-to-server stream using the given
    /// [`ServerConnector`].
    ///
    /// `jid` and `password` must be the user account's credentials. `jid` may
    /// either be a bare JID (to let the server choose a resource) or a full
    /// JID (to request a specific resource from the server, with no guarantee
    /// of success).
    ///
    /// `timeouts` controls the responsiveness to connection interruptions
    /// on the underlying transports. Please see the [`Timeouts`] struct's
    /// documentation for hints on how to correctly configure this.
    ///
    /// The `queue_depth` controls the sizes for the incoming and outgoing
    /// stanza queues. If the size is exceeded, the corresponding direction
    /// will block until the queues can be flushed. Note that the respective
    /// reverse direction is not affected (i.e. if your outgoing queue is
    /// full for example because of a slow server, you can still receive
    /// data).
    pub fn new_c2s<C: ServerConnector>(
        server: C,
        jid: Jid,
        password: String,
        timeouts: Timeouts,
        queue_depth: usize,
    ) -> Self {
        Self::new_c2s_with_ingress(server, jid, password, timeouts, queue_depth, None)
    }

    /// Establish a c2s stream with an optional durable ingress barrier.
    pub fn new_c2s_with_ingress<C: ServerConnector>(
        server: C,
        jid: Jid,
        password: String,
        timeouts: Timeouts,
        queue_depth: usize,
        ingress: Option<InboundCommit>,
    ) -> Self {
        Self::new_c2s_config(
            server,
            jid,
            password,
            timeouts,
            queue_depth,
            ingress,
            None,
            None,
        )
    }

    /// Resume an owned handoff on the same authenticated full JID.
    /// The caller must enforce server expiry and keep the original full JID.
    pub fn new_c2s_with_handoff<C: ServerConnector>(
        server: C,
        jid: xmpp_parsers::jid::FullJid,
        password: String,
        timeouts: Timeouts,
        queue_depth: usize,
        ingress: InboundCommit,
        handoff: DetachedState,
    ) -> std::io::Result<Self> {
        Self::validate_handoff(&handoff)?;
        Ok(Self::new_c2s_config(
            server,
            jid.into(),
            password,
            timeouts,
            queue_depth,
            Some(ingress),
            Some(handoff),
            None,
        ))
    }

    /// Establish or restore a c2s stream using actual protocol checkpoint context.
    /// The caller must enforce handoff age and preserve the authenticated full JID.
    pub fn new_c2s_with_checkpoint<C: ServerConnector>(
        server: C,
        jid: xmpp_parsers::jid::FullJid,
        password: String,
        timeouts: Timeouts,
        queue_depth: usize,
        commit: CheckpointCommit,
        handoff: Option<DetachedState>,
    ) -> std::io::Result<Self> {
        if let Some(ref state) = handoff {
            Self::validate_handoff(state)?;
        }
        Ok(Self::new_c2s_config(
            server,
            jid.into(),
            password,
            timeouts,
            queue_depth,
            None,
            handoff,
            Some(commit),
        ))
    }

    fn new_c2s_config<C: ServerConnector>(
        server: C,
        jid: Jid,
        password: String,
        timeouts: Timeouts,
        queue_depth: usize,
        ingress: Option<InboundCommit>,
        handoff: Option<DetachedState>,
        checkpoint: Option<CheckpointCommit>,
    ) -> Self {
        let tasks = Arc::new(ConnectorTasks::default());
        let captured_tasks = tasks.clone();
        let reconnector = Box::new(
            move |_preferred_location: Option<String>, slot: oneshot::Sender<Connection>| {
                let jid = jid.clone();
                let server = server.clone();
                let password = password.clone();
                let task = tokio::spawn(async move {
                    const MAX_DELAY: Duration = Duration::new(30, 0);
                    let mut delay = Duration::new(1, 0);
                    loop {
                        log::debug!("Starting new connection as {}", jid);
                        match crate::client::login::client_auth(
                            server.clone(),
                            jid.clone(),
                            password.clone(),
                            timeouts,
                        )
                        .await
                        {
                            Ok((features, stream)) => {
                                log::debug!("Connection as {} established", jid);
                                let stream = stream.box_stream();
                                let Err(mut conn) = slot.send(Connection {
                                    stream,
                                    features,
                                    identity: jid,
                                }) else {
                                    // Send succeeded, we're done here.
                                    return;
                                };

                                log::debug!("StanzaStream dropped, attempting graceful termination of fresh stream.");
                                // Send failed, i.e. the stanzastream is dead. Let's
                                // be polite and close this stream cleanly.
                                // We don't care whether that works, though, we
                                // just want to release the resources after a
                                // defined amount of time.
                                let _: Result<_, _> = tokio::time::timeout(
                                    LOCAL_SHUTDOWN_TIMEOUT,
                                    <XmppStream as SinkExt<&Stanza>>::close(&mut conn.stream),
                                )
                                .await;
                                return;
                            }
                            Err(e) => {
                                // TODO: auth errors should probably be fatal??
                                log::error!("Failed to connect: {}. Retrying in {:?}.", e, delay);
                                tokio::time::sleep(delay).await;
                                delay *= 2;
                                if delay > MAX_DELAY {
                                    delay = MAX_DELAY;
                                }
                            }
                        }
                    }
                });
                let mut tasks = captured_tasks
                    .0
                    .lock()
                    .expect("connector task lock poisoned");
                tasks.retain(|task| !task.is_finished());
                tasks.push(task);
            },
        );
        let mut stream = Self::new_config(reconnector, queue_depth, ingress, handoff, checkpoint);
        stream.connector_tasks = Some(tasks);
        stream
    }

    /// Create a new stanza stream.
    ///
    /// Stanza streams operate using a `connector` which is responsible for
    /// producing a new stream whenever necessary. It is the connector's
    /// responsibility that:
    ///
    /// - It never fails to send to the channel it is given. If the connector
    ///   drops the channel, the `StanzaStream` will consider this fatal and
    ///   fail the stream.
    ///
    /// - All streams are authenticated and secured as necessary.
    ///
    /// - All streams are authenticated for the same entity. If the connector
    ///   were to provide streams for different identities, information leaks
    ///   could occur as queues from previous sessions are being flushed on
    ///   the new stream on a reconnect.
    ///
    /// Most notably, the `connector` is **not** responsible for performing
    /// resource binding: Resource binding is handled by the `StanzaStream`.
    ///
    /// `connector` will be called soon after `new()` was called to establish
    /// the first underlying stream for the `StanzaStream`.
    ///
    /// The `queue_depth` controls the sizes for the incoming and outgoing
    /// stanza queues. If the size is exceeded, the corresponding direction
    /// will block until the queues can be flushed. Note that the respective
    /// reverse direction is not affected (i.e. if your outgoing queue is
    /// full for example because of a slow server, you can still receive
    /// data).
    pub fn new(
        connector: Box<dyn FnMut(Option<String>, oneshot::Sender<Connection>) + Send + 'static>,
        queue_depth: usize,
    ) -> Self {
        Self::new_with_ingress(connector, queue_depth, None)
    }

    /// Create a stream whose received stanzas pass a durable ingress barrier
    /// before delivery or any subsequent transport write. A failed commit
    /// terminates this stream without a footer or automatic reconnection.
    pub fn new_with_ingress(
        connector: Box<dyn FnMut(Option<String>, oneshot::Sender<Connection>) + Send + 'static>,
        queue_depth: usize,
        ingress: Option<InboundCommit>,
    ) -> Self {
        Self::new_config(connector, queue_depth, ingress, None, None)
    }

    /// Restore a handoff using a custom authenticated connector.
    /// It MUST produce the same authenticated full JID as the original session.
    /// The caller must reject expired handoffs before using this constructor.
    pub fn new_with_handoff(
        connector: Box<dyn FnMut(Option<String>, oneshot::Sender<Connection>) + Send + 'static>,
        queue_depth: usize,
        ingress: InboundCommit,
        handoff: DetachedState,
    ) -> std::io::Result<Self> {
        Self::validate_handoff(&handoff)?;
        Ok(Self::new_config(
            connector,
            queue_depth,
            Some(ingress),
            Some(handoff),
            None,
        ))
    }

    /// Custom-connector variant of the context-aware durability barrier.
    /// External connector tasks remain the caller's responsibility.
    pub fn new_with_checkpoint(
        connector: Box<dyn FnMut(Option<String>, oneshot::Sender<Connection>) + Send + 'static>,
        queue_depth: usize,
        commit: CheckpointCommit,
        handoff: Option<DetachedState>,
    ) -> std::io::Result<Self> {
        if let Some(ref state) = handoff {
            Self::validate_handoff(state)?;
        }
        Ok(Self::new_config(
            connector,
            queue_depth,
            None,
            handoff,
            Some(commit),
        ))
    }

    fn validate_handoff(handoff: &DetachedState) -> std::io::Result<()> {
        if handoff.sm.as_ref().is_some_and(|sm| {
            sm.unacked.len() >= (u32::MAX / 2) as usize
                || sm.id.as_ref().is_some_and(|id| id.is_empty())
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid SM handoff",
            ));
        }
        Ok(())
    }

    fn new_config(
        connector: Box<dyn FnMut(Option<String>, oneshot::Sender<Connection>) + Send + 'static>,
        queue_depth: usize,
        ingress: Option<InboundCommit>,
        handoff: Option<DetachedState>,
        checkpoint: Option<CheckpointCommit>,
    ) -> Self {
        // c2f = core to frontend, f2c = frontend to core
        let durable_ingress = ingress.is_some() || checkpoint.is_some();
        let (f2c_tx, c2f_rx, detach, worker) =
            StanzaStreamWorker::spawn(connector, queue_depth, ingress, handoff, checkpoint);
        Self {
            tx: f2c_tx,
            rx: c2f_rx,
            detach: Some(detach),
            worker,
            durable_ingress,
            connector_tasks: None,
        }
    }

    /// Stop socket I/O without an XML footer and return all outbound work.
    /// Requires a durable ingress callback: buffered frontend events can be
    /// discarded only because they already committed before delivery.
    /// This is not persistence or a cross-process resume operation by itself.
    pub async fn detach_quiet(mut self) -> std::io::Result<Option<DetachedState>> {
        if !self.durable_ingress {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quiet detach requires durable ingress",
            ));
        }
        if let Some(detach) = self.detach.take() {
            let _ = detach.send(());
        }
        let result = loop {
            // Draining removes frontend backpressure, so the worker can leave
            // a blocked channel send and observe the prioritized detach request.
            tokio::select! {
                biased;
                result = &mut self.worker => break result.map_err(|_| std::io::Error::other("XMPP stream worker failed")),
                event = self.rx.recv() => if event.is_none() {
                    break (&mut self.worker).await.map_err(|_| std::io::Error::other("XMPP stream worker failed"));
                },
            }
        };
        // The stream worker is gone, so it can no longer start a connector.
        // Abort and join every library-owned login/backoff task before handoff.
        if let Some(tasks) = self.connector_tasks.take() {
            let handles = tasks.take();
            for handle in &handles {
                handle.abort();
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
        result
    }

    async fn assert_send(&self, cmd: QueueEntry) {
        match self.tx.send(cmd).await {
            Ok(()) => (),
            Err(_) => panic!("Stream closed or the stream's background workers have crashed."),
        }
    }

    /// Close the stream.
    ///
    /// This will initiate a clean shutdown of the stream and will prevent and
    /// cancel any more reconnection attempts.
    pub async fn close(mut self) {
        drop(self.tx); // closes stream.
        while let Some(ev) = self.rx.recv().await {
            log::trace!("discarding event {:?} after stream closure", ev);
        }
    }

    /// Send a stanza via the stream.
    ///
    /// Note that completion of this function merely signals that the stanza
    /// has been enqueued successfully: it may be stuck in the transmission
    /// queue for quite a while if the stream is currently disconnected. The
    /// transmission progress can be observed via the returned
    /// [`StanzaToken`].
    ///
    /// # Panics
    ///
    /// If the stream has failed catastrophically (i.e. due to a software
    /// bug), this function may panic.
    pub async fn send(&self, stanza: Box<Stanza>) -> StanzaToken {
        let (queue_entry, token) = QueueEntry::tracked(stanza);
        self.assert_send(queue_entry).await;
        token
    }

    /// Split the stream into the [`StanzaSender`] and [`StanzaReceiver`].
    pub fn split(self) -> (StanzaSender, StanzaReceiver) {
        let stream = Arc::new(ReceiverMutex::new(self));

        let tx = StanzaSender(stream.clone());
        let rx = StanzaReceiver(stream);

        (tx, rx)
    }

    /// Reunite the [`StanzaSender`] and [`StanzaReceiver`] back into a single stream.
    ///
    /// # Panics
    ///
    /// This function panics if the `Sender` and `Receiver` don't come from
    /// the same [`Stream`].
    pub fn reunite(tx: StanzaSender, rx: StanzaReceiver) -> Self {
        assert!(
            Arc::ptr_eq(&tx.0, &rx.0),
            "Unrelated Sender and Receiver passed to reunite."
        );

        drop(tx);

        let inner = Arc::try_unwrap(rx.0).expect("Failed to unwrap Receiver Arc");
        inner.into_inner()
    }
}

impl Stream for StanzaStream {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// Split-stream mutex with a registered receiver wake on writer release.
/// The receiver never holds a lock future/reservation across cancellation or
/// reunite. Register and retry closes the unlock-before-register race.
pub(crate) struct ReceiverMutex<T> {
    inner: Mutex<T>,
    receiver: futures::task::AtomicWaker,
}
impl<T> std::fmt::Debug for ReceiverMutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceiverMutex { .. }")
    }
}
impl<T> ReceiverMutex<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            inner: Mutex::new(value),
            receiver: futures::task::AtomicWaker::new(),
        }
    }
    pub(crate) async fn lock(&self) -> ReceiverMutexGuard<'_, T> {
        ReceiverMutexGuard {
            guard: Some(self.inner.lock().await),
            receiver: &self.receiver,
        }
    }
    pub(crate) fn poll_lock(&self, cx: &mut Context<'_>) -> Poll<tokio::sync::MutexGuard<'_, T>> {
        if let Ok(guard) = self.inner.try_lock() {
            return Poll::Ready(guard);
        }
        self.receiver.register(cx.waker());
        match self.inner.try_lock() {
            Ok(guard) => {
                self.receiver.take();
                Poll::Ready(guard)
            }
            Err(_) => Poll::Pending,
        }
    }
    pub(crate) fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}
pub(crate) struct ReceiverMutexGuard<'a, T> {
    guard: Option<tokio::sync::MutexGuard<'a, T>>,
    receiver: &'a futures::task::AtomicWaker,
}
impl<T> std::ops::Deref for ReceiverMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_deref().expect("live split guard")
    }
}
impl<T> std::ops::DerefMut for ReceiverMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_deref_mut().expect("live split guard")
    }
}
impl<T> Drop for ReceiverMutexGuard<'_, T> {
    fn drop(&mut self) {
        // Wake only after releasing the mutex: waking while locked can cause
        // the receiver to observe contention and miss the final unlock again.
        drop(self.guard.take());
        self.receiver.wake();
    }
}

/// Send half of the [`StanzaStream`]
#[derive(Debug)]
pub struct StanzaSender(pub(super) Arc<ReceiverMutex<StanzaStream>>);

impl StanzaSender {
    /// Send a stanza via the stream.
    ///
    /// See the documentation of [`StanzaStream::send()`].
    pub async fn send(&self, stanza: Box<Stanza>) -> StanzaToken {
        self.0.lock().await.send(stanza).await
    }
}

/// Receive half of the [`StanzaStream`]
#[derive(Debug)]
pub struct StanzaReceiver(pub(super) Arc<ReceiverMutex<StanzaStream>>);

impl Stream for StanzaReceiver {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let mut stream = futures::ready!(self.0.poll_lock(cx));

        stream.rx.poll_recv(cx)
    }
}
