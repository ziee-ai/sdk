UPDATE groups
SET permissions = ARRAY[
  'profile::read', 'profile::edit',
  'chat::read', 'chat::create',
  'conversations::create', 'conversations::read', 'conversations::edit', 'conversations::delete',
  'messages::create', 'messages::read', 'messages::delete',
  'branches::create', 'branches::switch',
  'assistants::create', 'assistants::read', 'assistants::edit', 'assistants::delete',
  'mcp_servers::read', 'mcp_servers::create', 'mcp_servers::edit', 'mcp_servers::delete',
  'hub::models::read', 'hub::models::read_version', 'hub::models::download',
  'hub::assistants::read', 'hub::assistants::read_version', 'hub::assistants::create',
  'hub::mcp_servers::read', 'hub::mcp_servers::read_version', 'hub::mcp_servers::create',
  'files::read', 'files::upload', 'files::download', 'files::delete',
  'files::preview', 'files::generate_token',
  'user_llm_providers::read'
]
WHERE name = 'Users' AND is_system = TRUE AND is_default = TRUE;
