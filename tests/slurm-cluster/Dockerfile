FROM rockylinux:9

# Install EPEL and enable CRB for dependencies
RUN dnf -y install epel-release dnf-plugins-core && \
    dnf config-manager --set-enabled crb

# Install GSI Slurm repo
RUN dnf install -y http://cluster-mirror.hpc.gsi.de/packages/slurm-25.11/el9/slurm-release.rpm

# Install Slurm packages and dependencies
RUN dnf -y install \
        slurm \
        slurm-libs \
        slurm-slurmctld \
        slurm-slurmd \
        slurm-contribs \
        slurm-pam_slurm \
        munge \
        procps-ng \
        iproute \
        hostname \
        stress-ng \
    && dnf clean all

# Install tools for building jam inside container (separate step for --allowerasing)
RUN dnf -y --allowerasing install gcc curl && dnf clean all

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Create users via systemd-sysusers (munge) and manually (slurm)
RUN systemd-sysusers && \
    useradd -r -s /sbin/nologin slurm

# Create directories with proper ownership
RUN mkdir -p /var/spool/slurmctld /var/spool/slurmd /var/log/slurm /var/run/slurm /data \
             /etc/munge /var/lib/munge /var/log/munge /run/munge && \
    chown -R slurm:slurm /var/spool/slurmctld /var/spool/slurmd /var/log/slurm /var/run/slurm && \
    chown -R munge:munge /etc/munge /var/lib/munge /var/log/munge /run/munge

# Setup munge (shared key for all nodes, must be >= 32 bytes)
RUN dd if=/dev/urandom bs=1 count=1024 > /etc/munge/munge.key 2>/dev/null && \
    chown munge:munge /etc/munge/munge.key && \
    chmod 400 /etc/munge/munge.key

COPY scripts/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

WORKDIR /data
ENTRYPOINT ["/entrypoint.sh"]
