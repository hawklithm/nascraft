use crate::config::AppConfig;
use crate::display_remote::DLNAPlayer;
use crate::dlna_renderer::RendererManager;
use crate::upload::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppContext {
    pub app_state: Arc<AppState>,
    pub config: AppConfig,
    pub dlna_player: Arc<Mutex<DLNAPlayer>>,
    pub renderer_manager: Arc<RendererManager>,
}
