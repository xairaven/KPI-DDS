# AWS: one EC2 server with the lecturer's system in Docker Compose.

# Latest Ubuntu 26.04 LTS image published by Canonical.
data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical

  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-resolute-26.04-amd64-server-*"]
  }
}

# The network every AWS account has in each region; enough for one server.
data "aws_vpc" "default" {
  default = true
}

resource "aws_key_pair" "bms" {
  key_name   = "bms-cloud"
  public_key = local.ssh_public_key
}

# The firewall of the server. Everything not allowed here is dropped.
resource "aws_security_group" "hiveemulator" {
  name        = "bms-hiveemulator"
  description = "Hive Emulator of the lecturer"
  vpc_id      = data.aws_vpc.default.id
}

resource "aws_vpc_security_group_ingress_rule" "ssh" {
  security_group_id = aws_security_group.hiveemulator.id
  description       = "SSH from the admin"
  cidr_ipv4         = local.admin_cidr
  ip_protocol       = "tcp"
  from_port         = 22
  to_port           = 22
}

# Communication Control: bms-deconfliction (Azure) sends commands, bms-ingest
# polls the zones; the admin uses its Swagger UI, and the map in the admin's
# browser calls it directly.
resource "aws_vpc_security_group_ingress_rule" "cc" {
  for_each = {
    azure = "${local.azure_ip}/32"
    admin = local.admin_cidr
  }

  security_group_id = aws_security_group.hiveemulator.id
  description       = "Communication Control from ${each.key}"
  cidr_ipv4         = each.value
  ip_protocol       = "tcp"
  from_port         = 8080
  to_port           = 8080
}

# Redis: only bms-ingest (Azure) subscribes to the bus.
resource "aws_vpc_security_group_ingress_rule" "redis" {
  security_group_id = aws_security_group.hiveemulator.id
  description       = "Redis from Azure"
  cidr_ipv4         = "${local.azure_ip}/32"
  ip_protocol       = "tcp"
  from_port         = 6379
  to_port           = 6379
}

resource "aws_vpc_security_group_ingress_rule" "map" {
  security_group_id = aws_security_group.hiveemulator.id
  description       = "Web map of the lecturer, from the admin"
  cidr_ipv4         = local.admin_cidr
  ip_protocol       = "tcp"
  from_port         = 8081
  to_port           = 8081
}

resource "aws_vpc_security_group_egress_rule" "all" {
  security_group_id = aws_security_group.hiveemulator.id
  description       = "Outgoing traffic (images, packages)"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# Static public address. Separate from the server: it is known before the server
# exists (for Azure's firewall) and survives re-creating the server.
resource "aws_eip" "hiveemulator" {
  domain = "vpc"

  tags = {
    Name = "bms-hiveemulator"
  }
}

resource "aws_instance" "hiveemulator" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.aws_instance_type
  key_name               = aws_key_pair.bms.key_name
  vpc_security_group_ids = [aws_security_group.hiveemulator.id]

  # cloud-init runs this once, on the first boot: installs Docker and starts
  # the lecturer's system.
  user_data = templatefile("${path.module}/cloud-init/hiveemulator.yaml.tftpl", {
    redis_password = random_password.redis.result
    public_ip      = local.aws_ip
    image_owner    = var.image_owner
  })
  # A changed cloud-init only applies to a new server.
  user_data_replace_on_change = true

  root_block_device {
    volume_type = "gp3"
    volume_size = 20
  }

  # Token-only instance metadata (IMDSv2): protects the server's AWS
  # credentials endpoint from request forgery.
  metadata_options {
    http_tokens = "required"
  }

  tags = {
    Name = "bms-hiveemulator"
  }
}

resource "aws_eip_association" "hiveemulator" {
  instance_id   = aws_instance.hiveemulator.id
  allocation_id = aws_eip.hiveemulator.id
}
