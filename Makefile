CARGO ?= cargo
VM ?= xdp-lb

.PHONY: help
help:
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build: ## Build datapath and control plane (debug)
	$(CARGO) build

.PHONY: release
release: ## Build datapath and control plane (release)
	$(CARGO) build --release

.PHONY: test
test: ## Run control plane unit tests
	$(CARGO) test

.PHONY: test-datapath
test-datapath: ## Run datapath tests through BPF_PROG_TEST_RUN (needs root)
	$(CARGO) test --test datapath --no-run
	sudo $$($(CARGO) test --test datapath --no-run --message-format=json 2>/dev/null \
		| grep -o '"executable":"[^"]*datapath[^"]*"' | tail -1 | cut -d'"' -f4) \
		--include-ignored --test-threads=1

.PHONY: test-all
test-all: test test-datapath ## Run every test

.PHONY: lint
lint: ## Run clippy and rustfmt checks
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) fmt --check

.PHONY: deps
deps: ## Install build dependencies (Debian/Ubuntu)
	sudo apt-get update
	sudo apt-get install -y clang llvm libbpf-dev linux-headers-$$(uname -r) \
		build-essential pkg-config iproute2 python3 curl

.PHONY: netns-up
netns-up: ## Create the client/lb/backend network namespace test rig
	sudo ./test/netns-setup.sh

.PHONY: netns-dsr
netns-dsr: ## Convert the running rig to direct server return
	sudo ./test/netns-dsr.sh

.PHONY: netns-down
netns-down: ## Tear down the test rig
	sudo ./test/netns-teardown.sh

.PHONY: run
run: build ## Run the load balancer inside the lb namespace
	sudo ip netns exec lb ./target/debug/xdp-lb --config test/config.netns.yaml

.PHONY: smoke
smoke: ## Send traffic from the client namespace and print which backend answered
	sudo ./test/smoke.sh

.PHONY: vm
vm: ## Start the Lima Linux VM used for development from macOS
	limactl start --name=$(VM) lima/xdp-lb.yaml

.PHONY: shell
shell: ## Open a shell in the Lima VM at the project directory
	limactl shell $(VM) --workdir /Users/$$(whoami)/Documents/Portfolio/xdp-lb

.PHONY: clean
clean:
	$(CARGO) clean
