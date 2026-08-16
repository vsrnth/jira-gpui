use std::sync::Arc;

use jira_domain::{EventId, JiraSiteId, UpdateEvent};

use crate::{
    ApplicationError, ApplicationEvent, ApplicationEventSink, UpdateFeedPort, UpdateFeedQuery,
};

#[derive(Clone)]
pub struct UpdateFeedService {
    feed: Arc<dyn UpdateFeedPort>,
    events: Arc<dyn ApplicationEventSink>,
}

impl UpdateFeedService {
    pub fn new(feed: Arc<dyn UpdateFeedPort>, events: Arc<dyn ApplicationEventSink>) -> Self {
        Self { feed, events }
    }

    pub async fn list(
        &self,
        query: &UpdateFeedQuery,
    ) -> Result<Vec<UpdateEvent>, ApplicationError> {
        if !(1..=500).contains(&query.limit) {
            return Err(ApplicationError::invalid_input(
                "feed page size must be between 1 and 500",
            ));
        }
        self.feed.list(query).await
    }

    pub async fn unread_count(&self, site_id: &JiraSiteId) -> Result<usize, ApplicationError> {
        self.feed.unread_count(site_id).await
    }

    pub async fn mark_read(
        &self,
        site_id: &JiraSiteId,
        event_ids: &[EventId],
        read: bool,
    ) -> Result<usize, ApplicationError> {
        let changed = self.feed.mark_read(event_ids, read).await?;
        if changed > 0 {
            self.events.publish(ApplicationEvent::FeedChanged {
                site_id: site_id.clone(),
            });
        }
        Ok(changed)
    }

    pub async fn mark_all_read(&self, site_id: &JiraSiteId) -> Result<usize, ApplicationError> {
        let changed = self.feed.mark_all_read(site_id).await?;
        if changed > 0 {
            self.events.publish(ApplicationEvent::FeedChanged {
                site_id: site_id.clone(),
            });
        }
        Ok(changed)
    }
}
