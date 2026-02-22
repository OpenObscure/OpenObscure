#!/usr/bin/env python3
"""Sample Python file with hardcoded PII for detection testing."""

import os
import requests

# DANGER: Hardcoded API keys (should be in env vars or vault)
ANTHROPIC_API_KEY = "sk-ant-api03-HardcodedKey123456789012345678901234567890abc"
OPENAI_API_KEY = "sk-proj-abc123def456ghi789jklmnopqrstuvwxyz01234567890ABCDEF"
AWS_ACCESS_KEY = "AKIAIOSFODNN7EXAMPLE"
AWS_SECRET_KEY = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
GITHUB_TOKEN = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij1234"

# Database connection with PII in connection string
DATABASE_URL = "postgresql://admin:P@ssw0rd@10.0.0.55:5432/production"
REDIS_URL = "redis://10.0.0.60:6379/0"

# Slack notification config
SLACK_BOT_TOKEN = "xoxb-123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx"
SLACK_CHANNEL = "#alerts"

# Admin contact info embedded in code
ADMIN_EMAIL = "admin@meridian-tech.com"
ADMIN_PHONE = "415-555-0132"
SUPPORT_EMAIL = "support@openobscure.dev"


def notify_admin(message: str):
    """Send alert to admin — contains hardcoded PII."""
    requests.post(
        "https://hooks.slack.com/services/T00000000/B00000000/XXXX",
        json={
            "text": f"Alert from 10.0.0.55: {message}",
            "channel": SLACK_CHANNEL,
        },
        headers={"Authorization": f"Bearer {SLACK_BOT_TOKEN}"},
    )


def process_customer(customer_id: str):
    """Example function with PII in comments and debug output."""
    # Test customer: James Henderson, SSN 287-65-4321
    # Card on file: 4532015112830366
    # Contact: (206) 555-0312, j.henderson@email.com
    print(f"Processing customer {customer_id}")
    print(f"Server IP: 203.0.113.50, IPv6: 2001:db8:85a3::8a2e:370:7334")
    print(f"Device MAC: 3C:22:FB:7A:B1:90")
    print(f"GPS: 47.6062, -122.3321")


def create_test_user():
    """Seed function with test data — should never run in production."""
    return {
        "name": "Sarah Mitchell",
        "ssn": "378-22-9104",
        "email": "s.mitchell@gmail.com",
        "phone": "(503) 555-0147",
        "card": "5425233430109903",
        "address": "892 Oak Street, Portland, OR 97201",
        "ip": "198.51.100.17",
        "mac": "48:D7:05:F3:A2:81",
    }


if __name__ == "__main__":
    # Quick test with hardcoded credentials
    headers = {
        "x-api-key": ANTHROPIC_API_KEY,
        "Authorization": f"Bearer {OPENAI_API_KEY}",
    }
    process_customer("CUST-88412")
