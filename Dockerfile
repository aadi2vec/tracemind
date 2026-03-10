# ── Build stage ────────────────────────────────────────────────────────────
FROM python:3.11-slim AS base

# System deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Install Python deps first (layer-cached)
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy source
COPY . .

# Make data directories
RUN mkdir -p /app/logs /app/data

# ── Runtime ─────────────────────────────────────────────────────────────────
# Default: interactive CLI (start_agent.py). Override CMD in docker-compose
# for one-shot queries via run_agent.py.
CMD ["python", "start_agent.py"]
