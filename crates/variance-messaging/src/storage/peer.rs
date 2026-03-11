use crate::error::*;

use super::LocalMessageStorage;

impl LocalMessageStorage {
    pub(crate) async fn impl_store_peer_name(
        &self,
        did: &str,
        username: &str,
        discriminator: u32,
    ) -> Result<()> {
        let tree = self.peer_names_tree()?;
        let value = format!("{username}#{discriminator:04}");
        tree.insert(did.as_bytes(), value.as_bytes())
            .map_err(|e| Error::Storage { source: e })?;
        Ok(())
    }

    pub(crate) async fn impl_load_all_peer_names(&self) -> Result<Vec<(String, String, u32)>> {
        let tree = self.peer_names_tree()?;
        let mut result = Vec::new();
        for item in tree.iter() {
            let (k, v) = item.map_err(|e| Error::Storage { source: e })?;
            let did = String::from_utf8_lossy(&k).into_owned();
            let formatted = String::from_utf8_lossy(&v).into_owned();
            if let Some((username, disc_str)) = formatted.rsplit_once('#') {
                if let Ok(disc) = disc_str.parse::<u32>() {
                    result.push((did, username.to_string(), disc));
                }
            }
        }
        Ok(result)
    }
}
