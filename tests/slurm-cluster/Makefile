.PHONY: build up down logs status test test-jam shell clean deploy

CONTAINERS := slurmctld c1 c2
JAM_SRC := ../..

build:
	docker build --network=host -t jam-slurm-node .

up: build
	docker compose up -d
	@echo "Waiting for cluster to be ready..."
	@sleep 10
	@$(MAKE) status

down:
	docker compose down

logs:
	docker compose logs -f

status:
	docker compose exec slurmctld sinfo
	docker compose exec slurmctld scontrol show nodes

test:
	docker compose exec slurmctld srun -N2 hostname
	docker compose exec slurmctld sbatch --wrap="sleep 5 && hostname" -N1

test-jam:
	./test-jam.sh

shell:
	docker compose exec slurmctld bash

clean:
	docker compose down -v --rmi local

deploy:
	@echo "Copying source to container..."
	@docker exec slurmctld mkdir -p /data/jam-src
	@docker exec slurmctld find /data/jam-src -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} +
	@docker cp $(JAM_SRC)/src slurmctld:/data/jam-src/
	@docker cp $(JAM_SRC)/Cargo.toml slurmctld:/data/jam-src/
	@docker cp $(JAM_SRC)/Cargo.lock slurmctld:/data/jam-src/
	@echo "Building jam in container..."
	@docker exec slurmctld bash -c 'cd /data/jam-src && cargo build --release'
	@echo "Installing to all containers..."
	@for c in $(CONTAINERS); do \
		docker exec $$c cp /data/jam-src/target/release/jam /usr/local/bin/jam && \
		docker exec $$c chmod +x /usr/local/bin/jam; \
	done
	@echo "Done. jam installed to: $(CONTAINERS)"
