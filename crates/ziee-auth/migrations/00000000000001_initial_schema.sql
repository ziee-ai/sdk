-- =====================================================
-- Ziee Chat Initial Schema
-- Fresh implementation with axum-login authentication
-- =====================================================

-- =====================================================
-- 1. USERS TABLE
-- =====================================================
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username VARCHAR(100) NOT NULL UNIQUE,
    email VARCHAR(255) NOT NULL UNIQUE,
    email_verified BOOLEAN DEFAULT FALSE NOT NULL,
    password_hash VARCHAR(255), -- NULL for external auth only users
    display_name VARCHAR(255),
    avatar_url TEXT,
    is_active BOOLEAN DEFAULT TRUE NOT NULL,
    is_admin BOOLEAN DEFAULT FALSE NOT NULL, -- Root admin flag (only ONE can be true)
    permissions TEXT[] DEFAULT '{}' NOT NULL, -- Direct user-level permissions
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    last_login_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_is_active ON users(is_active);
CREATE INDEX idx_users_last_login_at ON users(last_login_at);
CREATE INDEX idx_users_permissions ON users USING GIN(permissions);

-- Partial unique index to ensure only ONE root admin (is_admin = true) can exist
CREATE UNIQUE INDEX unique_root_admin ON users (is_admin) WHERE is_admin = true;

-- =====================================================
-- 2. GROUPS TABLE
-- =====================================================
CREATE TABLE groups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) NOT NULL UNIQUE,
    description TEXT,
    permissions TEXT[] DEFAULT '{}' NOT NULL, -- PostgreSQL array of permission strings
    is_system BOOLEAN DEFAULT FALSE NOT NULL, -- System groups cannot be deleted
    is_active BOOLEAN DEFAULT TRUE NOT NULL, -- Inactive groups are ignored in permission checks
    is_default BOOLEAN DEFAULT FALSE NOT NULL, -- Default group for new user registrations
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL
);

CREATE INDEX idx_groups_name ON groups(name);
CREATE INDEX idx_groups_permissions ON groups USING GIN(permissions);

-- =====================================================
-- 3. USER_GROUPS (Many-to-Many)
-- =====================================================
CREATE TABLE user_groups (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    assigned_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    assigned_by UUID REFERENCES users(id),
    PRIMARY KEY (user_id, group_id)
);

CREATE INDEX idx_user_groups_user_id ON user_groups(user_id);
CREATE INDEX idx_user_groups_group_id ON user_groups(group_id);

-- =====================================================
-- 4. AUTH PROVIDERS
-- =====================================================
CREATE TABLE auth_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) NOT NULL UNIQUE,
    provider_type VARCHAR(50) NOT NULL, -- 'oauth2', 'saml', 'ldap'
    enabled BOOLEAN DEFAULT TRUE NOT NULL,
    config JSONB NOT NULL, -- Provider-specific configuration
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL
);

CREATE INDEX idx_auth_providers_enabled ON auth_providers(enabled);

-- =====================================================
-- 5. USER_AUTH_LINKS (External Identity Links)
-- =====================================================
CREATE TABLE user_auth_links (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider_id UUID NOT NULL REFERENCES auth_providers(id) ON DELETE CASCADE,
    external_id VARCHAR(255) NOT NULL, -- ID from external provider
    external_email VARCHAR(255),
    external_data JSONB, -- Store provider-specific user data
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    last_login_at TIMESTAMP WITH TIME ZONE,
    UNIQUE(provider_id, external_id)
);

CREATE INDEX idx_user_auth_links_user_id ON user_auth_links(user_id);
CREATE INDEX idx_user_auth_links_provider_id ON user_auth_links(provider_id);
CREATE INDEX idx_user_auth_links_external_id ON user_auth_links(provider_id, external_id);

-- =====================================================
-- 6. OAUTH_SESSIONS (Temporary OAuth Flow State)
-- =====================================================
CREATE TABLE oauth_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    state VARCHAR(255) NOT NULL UNIQUE,
    provider_id UUID NOT NULL REFERENCES auth_providers(id) ON DELETE CASCADE,
    pkce_verifier VARCHAR(255),
    nonce VARCHAR(255),
    redirect_uri TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL
);

CREATE INDEX idx_oauth_sessions_state ON oauth_sessions(state);
CREATE INDEX idx_oauth_sessions_expires_at ON oauth_sessions(expires_at);

-- =====================================================
-- 7. SESSIONS (tower-sessions will create this)
-- =====================================================
-- Note: tower-sessions-sqlx-store automatically creates its session table
-- We don't need to manually define it

-- =====================================================
-- 8. TRIGGERS
-- =====================================================
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER update_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_groups_updated_at
    BEFORE UPDATE ON groups
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_auth_providers_updated_at
    BEFORE UPDATE ON auth_providers
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_user_auth_links_updated_at
    BEFORE UPDATE ON user_auth_links
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- =====================================================
-- 9. DEFAULT DATA
-- =====================================================

-- Create Administrators system group (root admins with wildcard permission)
INSERT INTO groups (name, description, permissions, is_system, is_active)
VALUES (
    'Administrators',
    'System administrators with full access to all features',
    ARRAY['*'],
    TRUE,
    TRUE
);

-- Create default Users group for regular users
INSERT INTO groups (name, description, permissions, is_system, is_active, is_default)
VALUES (
    'Users',
    'Default group for all users',
    ARRAY['chat::read', 'chat::create', 'profile::read', 'profile::edit'],
    TRUE,
    TRUE,
    TRUE
);
