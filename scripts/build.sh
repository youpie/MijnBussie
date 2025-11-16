docker build -t mijn_bussie .
docker save mijn_bussie -o docker_images/mijn_bussie.tar

docker build -t mijn_bussie_auth -f ./auth/Dockerfile ./auth/repo
docker save mijn_bussie_auth -o docker_images/mijn_bussie_auth.tar