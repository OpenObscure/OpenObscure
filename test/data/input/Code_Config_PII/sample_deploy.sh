#!/bin/bash
# Deployment script — CONTAINS HARDCODED CREDENTIALS
# This is test data for PII detection — DO NOT USE in production

set -euo pipefail

# API keys (should be from vault, not hardcoded)
export ANTHROPIC_API_KEY="sk-ant-api03-DeployKey123456789012345678901234567890abc"
export AWS_ACCESS_KEY_ID="AKIAIOSFODNN7EXAMPLE"
export AWS_SECRET_ACCESS_KEY="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
export GITHUB_TOKEN="ghp_DeployScriptTokenShouldBeFromVault123456"

# Server details
SERVER_IP="10.0.0.55"
SERVER_IPV6="2001:db8:85a3::8a2e:370:7334"
DB_HOST="10.0.0.55"
CACHE_HOST="10.0.0.60"
PUBLIC_IP="203.0.113.50"

# Admin info
ADMIN_EMAIL="admin@meridian-tech.com"
ADMIN_PHONE="415-555-0132"

echo "Deploying to $SERVER_IP ($SERVER_IPV6)..."
echo "Admin contact: $ADMIN_EMAIL, $ADMIN_PHONE"

# Check server connectivity
ssh admin@${SERVER_IP} "hostname && uptime"

# Deploy application
ssh admin@${SERVER_IP} << 'REMOTE_SCRIPT'
    systemctl stop openobscure-proxy
    cd /opt/openobscure

    # Pull latest
    git pull https://ghp_DeployScriptTokenShouldBeFromVault123456@github.com/org/openobscure.git main

    # Update config with production values
    cat > /opt/openobscure/config.toml << EOF
    [server]
    bind = "0.0.0.0:8080"
    public_ip = "203.0.113.50"

    [database]
    host = "10.0.0.55"
    port = 5432

    [admin]
    email = "admin@meridian-tech.com"
    phone = "415-555-0132"
    EOF

    systemctl start openobscure-proxy
REMOTE_SCRIPT

# Verify deployment
curl -s https://${PUBLIC_IP}/health | jq .

# Notify via Slack
curl -X POST "https://hooks.slack.com/services/T0000/B0000/xxxx" \
    -H "Authorization: Bearer xoxb-123456789012-1234567890123-DeployNotifyToken" \
    -d "{\"text\": \"Deployed to ${SERVER_IP} by ${ADMIN_EMAIL}\"}"

# Log deployment for user j.henderson@company.com (SSN: 287-65-4321)
echo "Deployment complete. Notify j.henderson@company.com at (206) 555-0312"
echo "Server MAC: 00:1A:2B:3C:4D:5E, GPS: 47.6062, -122.3321"
