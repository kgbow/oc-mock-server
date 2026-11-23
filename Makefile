.PHONY: web build release test serve-offline clean

web:
	cd web && npm install && npm run build

build: web
	cargo build

release: web
	cargo build --release

test:
	cargo test --workspace

serve-offline: build
	./target/debug/oc-mock-server serve --dir $$(pwd)

clean:
	cargo clean
	rm -rf web/dist
