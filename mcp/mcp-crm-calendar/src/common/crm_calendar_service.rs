use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

use rmcp::{
    handler::server::common::Extension,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

use super::auth::{extract_caller, Caller};

// =================== ENVOLTORIO DE RESPUESTA (RASTRO DE AUDITORÍA) ===================

#[derive(Debug, Serialize)]
pub struct ToolResponse<T: Serialize> {
    pub output: T,
    pub called_by: String,
    pub roles: Vec<String>,
}

impl<T: Serialize> ToolResponse<T> {
    fn new(output: T, caller: &Caller) -> Self {
        Self {
            output,
            called_by: caller.banker_id.clone(),
            roles: caller.roles.clone(),
        }
    }

    fn into_call_tool_result(self) -> Result<CallToolResult, McpError> {
        let json = serde_json::to_string_pretty(&self).map_err(|e| {
            McpError::internal_error(format!("error serializando la respuesta: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

// =================== TIPOS DE DATOS ===================

#[derive(Debug, Serialize, FromRow)]
pub struct UpcomingMeeting {
    pub id: String,
    pub client_id: String,
    pub datetime: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct MeetingNotes {
    pub notes: Option<String>,
    pub client_id: String,
    pub datetime: DateTime<Utc>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetMeetingNotesParams {
    #[schemars(description = "Identificador de la reunión cuyas notas se quieren consultar")]
    pub meeting_id: String,
}

// =================== REGLA DE AISLAMIENTO ===================

/// Comprueba que `meeting_id` pertenece a `banker_id`. A diferencia de
/// `mcp-portfolio`/`mcp-kyc-compliance`, `meetings.banker_id` ya está en la
/// propia tabla: no hace falta un JOIN contra `clients` para validar
/// pertenencia. El mensaje de error es deliberadamente ambiguo: no debe
/// revelar si el `meeting_id` existe pero pertenece a otro banquero, o si no
/// existe en absoluto.
async fn assert_owns_meeting(
    pool: &PgPool,
    banker_id: &str,
    meeting_id: &str,
) -> Result<(), McpError> {
    let owner: Option<String> = sqlx::query_scalar("SELECT banker_id FROM meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

    match owner {
        Some(o) if o == banker_id => Ok(()),
        _ => {
            tracing::warn!(
                target: "tenant_violation",
                banker_id,
                meeting_id,
                "acceso denegado a reunión fuera de la agenda del llamante"
            );
            Err(McpError::invalid_params(
                "meeting_id no encontrado para el llamante autenticado",
                None,
            ))
        }
    }
}

// =================== SERVICIO MCP ===================

#[derive(Clone)]
pub struct CrmCalendarService {
    pool: PgPool,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl CrmCalendarService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Lista las próximas reuniones del banquero autenticado, ordenadas por fecha ascendente. No admite parámetros: el banker_id se toma siempre del JWT, nunca de un argumento del modelo. Es la herramienta que resuelve el client_id que necesitan las demás herramientas de preparación de reunión (p.ej. mcp-portfolio, mcp-kyc-compliance) — sin llamarla antes, el agente no sabe con quién es la próxima reunión."
    )]
    pub async fn get_upcoming_meetings(
        &self,
        Extension(parts): Extension<http::request::Parts>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);

        let meetings: Vec<UpcomingMeeting> = sqlx::query_as(
            "SELECT id, client_id, datetime FROM meetings \
             WHERE banker_id = $1 AND datetime > now() \
             ORDER BY datetime ASC",
        )
        .bind(&caller.banker_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| McpError::internal_error(format!("error de base de datos: {e}"), None))?;

        ToolResponse::new(meetings, &caller).into_call_tool_result()
    }

    #[tool(
        description = "Devuelve las notas, el cliente y la fecha de una reunión del banquero autenticado. Falla si la reunión no pertenece al llamante."
    )]
    pub async fn get_meeting_notes(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(params): Parameters<GetMeetingNotesParams>,
    ) -> Result<CallToolResult, McpError> {
        let caller = extract_caller(&parts);
        assert_owns_meeting(&self.pool, &caller.banker_id, &params.meeting_id).await?;

        let notes: Option<MeetingNotes> =
            sqlx::query_as("SELECT notes, client_id, datetime FROM meetings WHERE id = $1")
                .bind(&params.meeting_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("error de base de datos: {e}"), None)
                })?;

        let Some(notes) = notes else {
            return Err(McpError::invalid_params(
                "meeting_id no encontrado para el llamante autenticado",
                None,
            ));
        };

        ToolResponse::new(notes, &caller).into_call_tool_result()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CrmCalendarService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Servidor MCP de agenda para banqueros. Cada banquero solo puede consultar \
                 reuniones de su propia agenda: la identidad se toma del JWT validado por el \
                 sidecar Envoy, nunca de un parámetro del modelo. Herramientas disponibles: \
                 get_upcoming_meetings (sin parámetros, resuelve el client_id de la próxima \
                 reunión), get_meeting_notes(meeting_id).",
            )
            .with_server_info(Implementation::new(
                "mcp-crm-calendar".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ))
    }
}
