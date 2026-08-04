# Fedora workstation VM for running OpenShell demos against an OpenShift cluster.
#
# This Vagrantfile is specific to macOS on Intel (x86_64) using QEMU via
# vagrant-qemu. It will NOT work on Apple Silicon without changes (the box
# and CLI binaries are x86_64). For other setups, adapt the box, provider,
# and binary URLs to match your host architecture.
#
# Prerequisites:
#   brew install qemu        # or however you installed QEMU
#   vagrant plugin install vagrant-qemu
#
# Usage:
#   vagrant up
#   vagrant ssh
#   cd /vagrant/base
#
# The repo is synced to /vagrant inside the VM. Copy .env separately:
#   vagrant upload .env /vagrant/.env

Vagrant.configure("2") do |config|
  config.vm.box = "generic/fedora39"
  config.vm.hostname = "openshell-demos"

  config.vm.provider "qemu" do |qe|
    qe.memory = "4096"
    qe.smp = "2"
  end

  config.vm.synced_folder ".", "/vagrant", type: "rsync",
    rsync__exclude: [".git/", "old/", ".env"]

  config.vm.provision "shell", inline: <<-SHELL
    set -euo pipefail

    echo "==> Installing base packages..."
    dnf install -y --setopt=install_weak_deps=False \
      bash-completion jq openssl curl tar gzip

    echo "==> Installing OpenShift CLI (oc + kubectl)..."
    OC_VERSION="4.16"
    curl -sL "https://mirror.openshift.com/pub/openshift-v4/clients/ocp/stable-${OC_VERSION}/openshift-client-linux.tar.gz" \
      | tar xzf - -C /usr/local/bin oc kubectl
    oc version --client
    kubectl version --client --short 2>/dev/null || true

    echo "==> Installing Helm..."
    curl -sL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
    helm version --short

    echo "==> Installing OpenShell CLI..."
    OPENSHELL_VERSION="0.0.97"
    dnf install -y "https://github.com/NVIDIA/OpenShell/releases/download/v${OPENSHELL_VERSION}/openshell-${OPENSHELL_VERSION}-1.fc44.x86_64.rpm"
    openshell --version

    echo "==> Setting up bash completions..."
    COMP_DIR="/etc/bash_completion.d"
    oc completion bash > "${COMP_DIR}/oc"
    kubectl completion bash > "${COMP_DIR}/kubectl"
    helm completion bash > "${COMP_DIR}/helm"
    openshell completions bash > "${COMP_DIR}/openshell"

    echo "==> Done. 'vagrant ssh' to get in, repo is at /vagrant."
  SHELL
end
