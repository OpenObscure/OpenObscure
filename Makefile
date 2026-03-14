.PHONY: docker-slim docker-full docker-run-slim

# Build the slim image (no voice, no model files, ~80MB compressed)
docker-slim:
	docker build --target slim -t openobscure:slim .

# Build the full image (all models baked in, requires Git LFS)
docker-full:
	git lfs pull
	docker build --target full -t openobscure:full .

# Run the slim image using the key from the local keychain/env
docker-run-slim:
	docker run --rm \
	  -e OPENOBSCURE_MASTER_KEY=$$(openobscure print-key) \
	  -p 18790:18790 \
	  openobscure:slim
