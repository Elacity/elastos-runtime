//! Message channel implementation
//!
//! All inter-capsule messaging goes through the MessageChannel.
//! This ensures:
//! - Capability tokens are validated before delivery
//! - All messages are audited
//! - Rate limits are enforced
//! - Messages cannot be forged or spoofed

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::capability::token::{Action, CapabilityToken, ResourceId};
use crate::capability::CapabilityManager;
use crate::primitives::audit::AuditLog;
use crate::primitives::metrics::MetricsManager;
use crate::primitives::time::SecureTimestamp;

/// Maximum message size in bytes (1 MB)
pub const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

/// Maximum queued messages per capsule
pub const MAX_QUEUE_SIZE: usize = 1000;

/// Unique message identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(pub [u8; 16]);

impl MessageId {
    pub fn new() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// A message between capsules
#[derive(Debug, Clone)]
pub struct Message {
    /// Unique message ID
    pub id: MessageId,
    /// Source capsule ID
    pub from: String,
    /// Destination capsule ID
    pub to: String,
    /// Message payload (opaque bytes)
    pub payload: Vec<u8>,
    /// Timestamp when message was sent
    pub timestamp: SecureTimestamp,
    /// Optional reply-to message ID (for request/response patterns)
    pub reply_to: Option<MessageId>,
}

impl Message {
    /// Create a new message
    pub fn new(from: String, to: String, payload: Vec<u8>) -> Self {
        Self {
            id: MessageId::new(),
            from,
            to,
            payload,
            timestamp: SecureTimestamp::now(),
            reply_to: None,
        }
    }

    /// Create a reply to another message
    pub fn reply(original: &Message, from: String, payload: Vec<u8>) -> Self {
        Self {
            id: MessageId::new(),
            from,
            to: original.from.clone(),
            payload,
            timestamp: SecureTimestamp::now(),
            reply_to: Some(original.id.clone()),
        }
    }

    /// Get payload size in bytes
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

/// Error types for messaging
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageError {
    /// No capability to send to this destination
    NotAuthorized,
    /// Message exceeds size limit
    MessageTooLarge { size: usize, max: usize },
    /// Destination queue is full
    QueueFull,
    /// Destination capsule not found
    DestinationNotFound,
    /// Sender capsule not registered
    SenderNotRegistered,
    /// Rate limit exceeded
    RateLimitExceeded,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageError::NotAuthorized => write!(f, "not authorized to send message"),
            MessageError::MessageTooLarge { size, max } => {
                write!(f, "message too large: {} bytes (max {})", size, max)
            }
            MessageError::QueueFull => write!(f, "destination queue is full"),
            MessageError::DestinationNotFound => write!(f, "destination capsule not found"),
            MessageError::SenderNotRegistered => write!(f, "sender capsule not registered"),
            MessageError::RateLimitExceeded => write!(f, "message rate limit exceeded"),
        }
    }
}

impl std::error::Error for MessageError {}

/// Inbox for a single capsule
struct CapsuleInbox {
    /// Queued messages
    queue: VecDeque<Message>,
    /// Notification channel
    notify: mpsc::Sender<()>,
}

/// Central message routing channel
pub struct MessageChannel {
    /// Per-capsule inboxes
    inboxes: RwLock<HashMap<String, CapsuleInbox>>,
    /// Capability manager for authorization
    capability_manager: Arc<CapabilityManager>,
    /// Metrics manager for rate limiting
    metrics: Arc<MetricsManager>,
    /// Audit log
    audit_log: Arc<AuditLog>,
    /// Shell capsule ID — only this capsule is exempt from capability checks
    shell_id: RwLock<Option<String>>,
}

impl MessageChannel {
    /// Create a new message channel
    pub fn new(
        capability_manager: Arc<CapabilityManager>,
        metrics: Arc<MetricsManager>,
        audit_log: Arc<AuditLog>,
    ) -> Self {
        Self {
            inboxes: RwLock::new(HashMap::new()),
            capability_manager,
            metrics,
            audit_log,
            shell_id: RwLock::new(None),
        }
    }

    /// Set the shell capsule ID (only this capsule may send without a token)
    pub async fn set_shell_id(&self, id: String) {
        let mut shell_id = self.shell_id.write().await;
        *shell_id = Some(id);
    }

    /// Register a capsule to receive messages
    /// Returns a receiver that will be notified when messages arrive
    pub async fn register(&self, capsule_id: &str) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel(100);
        let inbox = CapsuleInbox {
            queue: VecDeque::new(),
            notify: tx,
        };

        let mut inboxes = self.inboxes.write().await;
        inboxes.insert(capsule_id.to_string(), inbox);

        tracing::debug!("Registered capsule {} for messaging", capsule_id);
        rx
    }

    /// Unregister a capsule (on shutdown)
    pub async fn unregister(&self, capsule_id: &str) {
        let mut inboxes = self.inboxes.write().await;
        inboxes.remove(capsule_id);
        tracing::debug!("Unregistered capsule {} from messaging", capsule_id);
    }

    /// Send a message from one capsule to another
    ///
    /// If a capability token is provided, it is validated before delivery.
    /// Pass None for token when the sender is the shell (exempt from capability checks).
    pub async fn send(
        &self,
        message: Message,
        token: Option<&CapabilityToken>,
    ) -> Result<MessageId, MessageError> {
        // Validate message size
        if message.size() > MAX_MESSAGE_SIZE {
            return Err(MessageError::MessageTooLarge {
                size: message.size(),
                max: MAX_MESSAGE_SIZE,
            });
        }

        // Enforce rate limit
        if self.metrics.would_exceed_message_limit(&message.from) {
            tracing::warn!("Capsule {} exceeded message rate limit", message.from);
            return Err(MessageError::RateLimitExceeded);
        }

        // Capability validation — shell exempt, everyone else needs a token
        let is_shell = {
            let shell_id = self.shell_id.read().await;
            shell_id.as_deref() == Some(&message.from)
        };

        if !is_shell {
            let cap_token = token.ok_or(MessageError::NotAuthorized)?;
            let resource = ResourceId::new(format!("elastos://message/{}", message.to));
            if let Err(e) = self
                .capability_manager
                .validate(cap_token, &message.from, Action::Message, &resource, None)
                .await
            {
                self.audit_log.security_warning(
                    "message_capability_denied",
                    &format!(
                        "Capsule {} denied messaging to {}: {}",
                        message.from, message.to, e
                    ),
                );
                return Err(MessageError::NotAuthorized);
            }
        }

        // Check destination exists
        {
            let inboxes = self.inboxes.read().await;
            if !inboxes.contains_key(&message.to) {
                return Err(MessageError::DestinationNotFound);
            }
            if !inboxes.contains_key(&message.from) {
                return Err(MessageError::SenderNotRegistered);
            }
        }

        // Deliver message
        let message_id = message.id.clone();
        {
            let mut inboxes = self.inboxes.write().await;
            if let Some(inbox) = inboxes.get_mut(&message.to) {
                // Check queue size
                if inbox.queue.len() >= MAX_QUEUE_SIZE {
                    return Err(MessageError::QueueFull);
                }

                inbox.queue.push_back(message.clone());

                // Notify recipient (ignore if channel is full)
                let _ = inbox.notify.try_send(());
            }
        }

        // Record metrics
        self.metrics.record_message_sent(&message.from);

        // Audit log
        self.audit_log
            .message_sent(&message.from, &message.to, message.size());

        Ok(message_id)
    }

    /// Receive messages for a capsule
    ///
    /// Returns all queued messages and clears the queue.
    pub async fn receive(&self, capsule_id: &str) -> Vec<Message> {
        let mut inboxes = self.inboxes.write().await;
        if let Some(inbox) = inboxes.get_mut(capsule_id) {
            let messages: Vec<Message> = inbox.queue.drain(..).collect();

            // Record metrics for each received message
            for _ in &messages {
                self.metrics.record_message_received(capsule_id);
            }

            messages
        } else {
            Vec::new()
        }
    }

    /// Receive a single message (oldest first)
    pub async fn receive_one(&self, capsule_id: &str) -> Option<Message> {
        let mut inboxes = self.inboxes.write().await;
        if let Some(inbox) = inboxes.get_mut(capsule_id) {
            let message = inbox.queue.pop_front();
            if message.is_some() {
                self.metrics.record_message_received(capsule_id);
            }
            message
        } else {
            None
        }
    }

    /// Check how many messages are queued for a capsule
    pub async fn queue_size(&self, capsule_id: &str) -> usize {
        let inboxes = self.inboxes.read().await;
        inboxes
            .get(capsule_id)
            .map(|inbox| inbox.queue.len())
            .unwrap_or(0)
    }

    /// Check if a capsule is registered
    pub async fn is_registered(&self, capsule_id: &str) -> bool {
        let inboxes = self.inboxes.read().await;
        inboxes.contains_key(capsule_id)
    }

    /// Get list of all registered capsules
    pub async fn list_registered(&self) -> Vec<String> {
        let inboxes = self.inboxes.read().await;
        inboxes.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityStore;

    fn create_test_channel() -> MessageChannel {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        // Start metrics for test capsules
        metrics.start_capsule("capsule-a");
        metrics.start_capsule("capsule-b");

        MessageChannel::new(capability_manager, metrics, audit_log)
    }

    /// Helper: create a test channel with capsule-a registered as the shell
    async fn create_test_channel_with_shell() -> MessageChannel {
        let channel = create_test_channel();
        channel.set_shell_id("capsule-a".to_string()).await;
        channel
    }

    #[tokio::test]
    async fn test_register_unregister() {
        let channel = create_test_channel();

        let _rx = channel.register("test-capsule").await;
        assert!(channel.is_registered("test-capsule").await);

        channel.unregister("test-capsule").await;
        assert!(!channel.is_registered("test-capsule").await);
    }

    #[tokio::test]
    async fn test_send_receive() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"hello world".to_vec(),
        );

        let msg_id = channel.send(message, None).await.unwrap();

        // Check queue size
        assert_eq!(channel.queue_size("capsule-b").await, 1);

        // Receive
        let messages = channel.receive("capsule-b").await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, msg_id);
        assert_eq!(messages[0].payload, b"hello world");
        assert_eq!(messages[0].from, "capsule-a");

        // Queue should be empty now
        assert_eq!(channel.queue_size("capsule-b").await, 0);
    }

    #[tokio::test]
    async fn test_message_too_large() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        let large_payload = vec![0u8; MAX_MESSAGE_SIZE + 1];
        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            large_payload,
        );

        let result = channel.send(message, None).await;
        assert!(matches!(result, Err(MessageError::MessageTooLarge { .. })));
    }

    #[tokio::test]
    async fn test_destination_not_found() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;

        let message = Message::new(
            "capsule-a".to_string(),
            "nonexistent".to_string(),
            b"test".to_vec(),
        );

        let result = channel.send(message, None).await;
        assert!(matches!(result, Err(MessageError::DestinationNotFound)));
    }

    #[tokio::test]
    async fn test_sender_not_registered() {
        let channel = create_test_channel_with_shell().await;

        let _rx_b = channel.register("capsule-b").await;

        // capsule-a is the shell but not registered — should get SenderNotRegistered
        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"test".to_vec(),
        );

        let result = channel.send(message, None).await;
        assert!(matches!(result, Err(MessageError::SenderNotRegistered)));
    }

    #[tokio::test]
    async fn test_reply_message() {
        let channel = create_test_channel();
        // Set both capsules as shell for this test (both send with None)
        channel.set_shell_id("capsule-a".to_string()).await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // Send original message (capsule-a is shell)
        let original = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"request".to_vec(),
        );
        channel.send(original.clone(), None).await.unwrap();

        // Receive and reply — capsule-b is not shell, so it needs a token
        // For simplicity, set capsule-b as shell for the reply
        channel.set_shell_id("capsule-b".to_string()).await;

        let received = channel.receive_one("capsule-b").await.unwrap();
        let reply = Message::reply(&received, "capsule-b".to_string(), b"response".to_vec());

        channel.send(reply, None).await.unwrap();

        // Check reply
        let reply_msg = channel.receive_one("capsule-a").await.unwrap();
        assert_eq!(reply_msg.reply_to, Some(original.id));
        assert_eq!(reply_msg.payload, b"response");
    }

    #[tokio::test]
    async fn test_queue_size_limit() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // Fill the queue (capsule-a is shell)
        for i in 0..MAX_QUEUE_SIZE {
            let message = Message::new(
                "capsule-a".to_string(),
                "capsule-b".to_string(),
                format!("message {}", i).into_bytes(),
            );
            channel.send(message, None).await.unwrap();
        }

        // Next message should fail
        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"overflow".to_vec(),
        );
        let result = channel.send(message, None).await;
        assert!(matches!(result, Err(MessageError::QueueFull)));
    }

    #[tokio::test]
    async fn test_rate_limit_enforcement() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // Set very low rate limit: 1 msg/sec = 60 msgs per minute period
        channel.metrics.set_limits(
            "capsule-a",
            crate::primitives::metrics::ResourceLimits {
                max_messages_per_sec: 1,
                ..Default::default()
            },
        );

        // Manually record enough messages to exceed the limit
        for _ in 0..60 {
            channel.metrics.record_message_sent("capsule-a");
        }

        // Next send should be rate limited
        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"should be rate limited".to_vec(),
        );
        let result = channel.send(message, None).await;
        assert!(matches!(result, Err(MessageError::RateLimitExceeded)));
    }

    #[tokio::test]
    async fn test_send_with_valid_token() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        metrics.start_capsule("capsule-a");
        metrics.start_capsule("capsule-b");

        let channel = MessageChannel::new(capability_manager.clone(), metrics, audit_log);

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // Grant a messaging capability token
        let token = capability_manager.grant(
            "capsule-a",
            ResourceId::new("elastos://message/capsule-b"),
            Action::Message,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"authorized message".to_vec(),
        );

        let result = channel.send(message, Some(&token)).await;
        assert!(result.is_ok(), "Valid token should allow message send");
    }

    #[tokio::test]
    async fn test_send_with_wrong_resource_token() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        metrics.start_capsule("capsule-a");
        metrics.start_capsule("capsule-b");

        let channel = MessageChannel::new(capability_manager.clone(), metrics, audit_log);

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // Grant token for wrong destination
        let token = capability_manager.grant(
            "capsule-a",
            ResourceId::new("elastos://message/capsule-c"),
            Action::Message,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"should be rejected".to_vec(),
        );

        let result = channel.send(message, Some(&token)).await;
        assert!(
            matches!(result, Err(MessageError::NotAuthorized)),
            "Wrong-resource token should be rejected"
        );
    }

    #[tokio::test]
    async fn test_send_with_wrong_action_token() {
        let store = Arc::new(CapabilityStore::new());
        let audit_log = Arc::new(AuditLog::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            store,
            audit_log.clone(),
            metrics.clone(),
        ));

        metrics.start_capsule("capsule-a");
        metrics.start_capsule("capsule-b");

        let channel = MessageChannel::new(capability_manager.clone(), metrics, audit_log);

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // Grant a read token (wrong action for messaging)
        let token = capability_manager.grant(
            "capsule-a",
            ResourceId::new("elastos://message/capsule-b"),
            Action::Read,
            crate::capability::token::TokenConstraints::default(),
            None,
        );

        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"should be rejected".to_vec(),
        );

        let result = channel.send(message, Some(&token)).await;
        assert!(
            matches!(result, Err(MessageError::NotAuthorized)),
            "Wrong-action token should be rejected"
        );
    }

    #[tokio::test]
    async fn test_send_without_token_allowed_for_shell() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // No token (None) — capsule-a is the shell, so allowed
        let message = Message::new(
            "capsule-a".to_string(),
            "capsule-b".to_string(),
            b"shell message".to_vec(),
        );

        let result = channel.send(message, None).await;
        assert!(
            result.is_ok(),
            "Shell should be allowed to send without token"
        );
    }

    #[tokio::test]
    async fn test_send_without_token_rejected_for_non_shell() {
        let channel = create_test_channel_with_shell().await;

        let _rx_a = channel.register("capsule-a").await;
        let _rx_b = channel.register("capsule-b").await;

        // No token (None) — capsule-b is NOT the shell, so rejected
        let message = Message::new(
            "capsule-b".to_string(),
            "capsule-a".to_string(),
            b"unauthorized message".to_vec(),
        );

        let result = channel.send(message, None).await;
        assert!(
            matches!(result, Err(MessageError::NotAuthorized)),
            "Non-shell without token should be rejected"
        );
    }
}
