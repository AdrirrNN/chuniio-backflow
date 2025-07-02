BUILD_TARGETS := x86_64-pc-windows-gnu i686-pc-windows-gnu

.PHONY: all build clean

all: build

build:
	@echo "Building for $(BUILD_TARGETS)"
	@for target in $(BUILD_TARGETS); do \
		echo "Building for $$target..."; \
		cargo build --release --target $$target; \
	done