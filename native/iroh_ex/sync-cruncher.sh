rsync --progress -rte "ssh -p 22" \
  --delete \
  --exclude .vscode --exclude .cargo --exclude target \
  --exclude .DS_Store \
  ./ 192.168.1.195:projects/2025_sensocto/checkouts/iroh_ex/native/iroh_ex
