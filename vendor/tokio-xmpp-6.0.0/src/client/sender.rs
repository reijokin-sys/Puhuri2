// Modified for Puhuri 2, 2026-10-24: register split receiver unlock wakes.
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025 xmpp-rs contributors.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use crate::stanzastream::ReceiverMutex;
use crate::stanzastream::StanzaToken;
use crate::IqRequest;
use crate::IqResponseToken;
use crate::Stanza;
use std::io;
use std::sync::Arc;
use xmpp_parsers::jid::Jid;

/// Write half of a [`Client`](crate::Client).
#[derive(Debug)]
pub struct ClientSender(pub(super) Arc<ReceiverMutex<super::Client>>);

impl ClientSender {
    /// Send a stanza.
    ///
    /// See the documentation of [`Client::send_stanza`](crate::Client::send_stanza) for more
    /// information.
    pub async fn send_stanza(&self, stanza: Stanza) -> Result<StanzaToken, io::Error> {
        self.0.lock().await.send_stanza(stanza).await
    }

    /// Send in iq.
    ///
    /// See the documentation of [`Client::send_iq`](crate::Client::send_iq) for more information.
    pub async fn send_iq(&self, to: Option<Jid>, req: IqRequest) -> IqResponseToken {
        self.0.lock().await.send_iq(to, req).await
    }
}
