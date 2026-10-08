use crate::config::AppConfig;
use crate::dlna_renderer::RendererManager;
use crate::upload::AppState;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppContext {
    pub app_state: Arc<AppState>,
    pub config: AppConfig,
    pub renderer_manager: Arc<RendererManager>,
}
