//! Host deletion reservations serialize cleanup with worker registration and
//! controls. Resource storage remains owned by FileStore.
use super::*;
use std::collections::BTreeMap;

pub(crate) struct KernelDeletion<'a> {
    runs: &'a KernelRuns,
    resources: BTreeMap<String, Resource>,
}

impl Drop for KernelDeletion<'_> {
    fn drop(&mut self) {
        let mut deleting = self.runs.deleting.lock().unwrap();
        for id in self.resources.keys() {
            deleting.remove(id);
        }
    }
}

impl KernelRuns {
    pub(super) fn ensure_not_deleting(&self, id: &str) -> Result<(), ApplicationError> {
        if self.deleting.lock().unwrap().contains(id) {
            return Err(ApplicationError::Conflict(
                "that Run is being deleted".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn reserve_deletion(
        &self,
        ids: &[String],
    ) -> Result<KernelDeletion<'_>, ApplicationError> {
        let active = self.active.lock().unwrap();
        let mut deleting = self.deleting.lock().unwrap();
        let mut resources = BTreeMap::new();
        for id in ids {
            if active.contains_key(id) {
                return Err(ApplicationError::Conflict(
                    "that Run is still running".into(),
                ));
            }
            if deleting.contains(id) {
                return Err(ApplicationError::Conflict(
                    "that Run is being deleted".into(),
                ));
            }
            let resource = self.load(id)?.ok_or(ApplicationError::Missing)?;
            resources.insert(id.clone(), resource);
        }
        deleting.extend(resources.keys().cloned());
        Ok(KernelDeletion {
            runs: self,
            resources,
        })
    }
}

impl KernelDeletion<'_> {
    pub(crate) fn record(&self, id: &str) -> Result<Option<GraphRunRecord>, ApplicationError> {
        self.resources.get(id).map(KernelRuns::record).transpose()
    }

    pub(crate) fn remove(&self, id: &str) -> Result<(), ApplicationError> {
        let resource = self.resources.get(id).ok_or(ApplicationError::Missing)?;
        self.runs.clear_pause(id)?;
        self.runs.observations.remove(id)?;
        self.runs
            .kernel
            .store()
            .remove(&resource.id)
            .map_err(|error| ApplicationError::Storage(error.to_string()))
    }
}

#[cfg(test)]
mod tests;
