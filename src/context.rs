use crate::display_remote::DLNAPlayer;
use crate::upload::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppContext {
    pub app_state: Arc<AppState>,
    pub dlna_player: Arc<Mutex<DLNAPlayer>>,
}
