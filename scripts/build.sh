#!/bin/bash

REMOTE_HOST="emma@babette"              # <-- change this
REMOTE_IMAGES_DIR="~/Images"
REMOTE_SERVICE_DIR="~/Services/MijnBussie"
LOCAL_IMAGES_DIR="docker_images"

mkdir -p "$LOCAL_IMAGES_DIR"

build_m() {
    echo "Building mijn_bussie..."
    docker build -t mijn_bussie .
    docker save mijn_bussie -o "$LOCAL_IMAGES_DIR/mijn_bussie.tar"
}

build_a() {
    echo "Building mijn_bussie_auth..."
    docker build -t mijn_bussie_auth -f ./auth/Dockerfile ./auth/repo
    docker save mijn_bussie_auth -o "$LOCAL_IMAGES_DIR/mijn_bussie_auth.tar"
}

transfer_m() {
    echo "Transferring mijn_bussie.tar..."
    scp "$LOCAL_IMAGES_DIR/mijn_bussie.tar" "$REMOTE_HOST:$REMOTE_IMAGES_DIR/"
}

transfer_a() {
    echo "Transferring mijn_bussie_auth.tar..."
    scp "$LOCAL_IMAGES_DIR/mijn_bussie_auth.tar" "$REMOTE_HOST:$REMOTE_IMAGES_DIR/"
}

remote_reload() {
    echo "Reloading containers on remote machine..."
    ssh "$REMOTE_HOST" "
        docker load -i $REMOTE_IMAGES_DIR/mijn_bussie_auth.tar 2>/dev/null || true
        docker load -i $REMOTE_IMAGES_DIR/mijn_bussie.tar 2>/dev/null || true
        cd $REMOTE_SERVICE_DIR
        sudo docker compose down
        sudo docker compose up -d
    "
}

# -------- ARGUMENT HANDLING -------- #
case "$1" in
    -a)
        build_a
        transfer_a
        remote_reload
        ;;
    -m)
        build_m
        transfer_m
        remote_reload
        ;;
    "" )
        # No arguments: do BOTH
        build_m
        build_a
        transfer_m
        transfer_a
        remote_reload
        ;;
    *)
        echo "Usage: $0 [-a | -m]"
        echo "  -a   Only build, transfer and deploy mijn_bussie_auth"
        echo "  -m   Only build, transfer and deploy mijn_bussie"
        exit 1
        ;;
esac

echo "Done!"
