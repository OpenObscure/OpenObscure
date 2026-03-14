# OpenObscure — HashiCorp Vault Agent sidecar configuration
#
# Pattern: Vault Agent writes the FPE key to /run/secrets/openobscure-master-key
# before OpenObscure starts. OpenObscure picks it up via step 2 of the key
# resolution chain (no code changes needed).
#
# Usage:
#   vault agent -config=deploy/vault/vault-agent-config.hcl
#
# See also: deploy/vault/docker-compose-vault.yml for a Docker Compose example.

vault {
  address = "https://vault.example.com"
}

auto_auth {
  method "aws" {
    config = {
      role = "openobscure-gateway"
    }
  }

  # Alternative: AppRole for non-AWS environments
  # method "approle" {
  #   config = {
  #     role_id_file_path   = "/etc/vault/role_id"
  #     secret_id_file_path = "/etc/vault/secret_id"
  #   }
  # }
}

# Write FPE key to the standard /run/secrets path that OpenObscure checks automatically.
template {
  source      = "/etc/vault/templates/fpe-key.tpl"
  destination = "/run/secrets/openobscure-master-key"
  perms       = "0400"

  # Restart OpenObscure if the key rotates (Vault dynamic secrets).
  # Remove this block if using a static key.
  exec {
    command = ["kill", "-HUP", "1"]
    timeout = "5s"
  }
}
