.PHONY: build up down logs status test shell clean

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

shell:
	docker compose exec slurmctld bash

clean:
	docker compose down -v --rmi local
