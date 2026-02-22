# OpenObscure API Setup Guide

## Quick Start

Install the proxy and configure your API keys:

```bash
export ANTHROPIC_API_KEY=sk-ant-api03-DocExample123456789012345678901234567890abc
export OPENAI_API_KEY=sk-DocExampleOpenAIKey1234567890abcdef
export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE
```

## Configuration

Create a config file at `~/.openobscure/config.yaml`:

```yaml
server:
  host: 10.0.0.55
  port: 8080
  public_ip: 203.0.113.50

api_keys:
  anthropic: sk-ant-api03-YourKeyHere123456789012345678901234567890abc
  github: ghp_YourGitHubTokenHere123456789012345678ab
```

## Python Client Example

```python
import anthropic

client = anthropic.Client(
    api_key="sk-ant-api03-PythonExample123456789012345678901234567890abc"
)

response = client.messages.create(
    model="claude-sonnet-4-20250514",
    messages=[{"role": "user", "content": "Hello"}],
)
```

## Network Setup

The proxy runs on `10.0.0.55:8080` with TLS termination. Internal services:

| Service    | IPv4        | IPv6                                  | MAC               |
|------------|-------------|---------------------------------------|-------------------|
| Web proxy  | 203.0.113.50| 2001:db8:85a3::8a2e:370:7334         | 00:1A:2B:3C:4D:5E |
| Database   | 10.0.0.55   | 2001:db8:1::30                        | 48:D7:05:F3:A2:81 |
| Cache      | 10.0.0.60   | 2001:db8:1::40                        | 3C:22:FB:7A:B1:90 |

## Troubleshooting

If you see connection errors, verify the server at `10.0.0.55:8080` is running.
Contact support at support@openobscure.dev or call (415) 555-0132.

For the inline code path, check that `ANTHROPIC_API_KEY=sk-ant-api03-InlineCodeKey12345` is set.

Admin email: admin@meridian-tech.com. GPS office: 47.6062, -122.3321.

## Slack Integration

Configure the Slack bot token:

```
SLACK_BOT_TOKEN=xoxb-123456789012-1234567890123-DocExampleSlackToken
```

Webhook URL: `https://hooks.slack.com/services/T0000/B0000/xoxb-DocExampleWebhook`
