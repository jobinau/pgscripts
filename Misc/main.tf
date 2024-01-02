# aws configure
# aws ec2 create-key-pair --key-name pgdemo --query KeyMaterial --output text > ~/.ssh/pgdemo.pem
# chmod 0600 ~/.ssh/pgdemo.pem
# ssh-keygen -p -f ~/.ssh/pgdemo.pem
# terraform plan
# terraform apply
# terraform state list ; terraform state show aws_instance.jobin-web-instance
terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

# Configure the AWS Provider
provider "aws" {
  region = "us-east-1"
}

resource "aws_vpc" "jobin_vpc" {
  cidr_block = "10.0.0.0/16"
  tags = {
    Name = "jobin-jumphost"
    Comment = "learning terraform automations for support needs"
  }
}

resource "aws_subnet" "jobin_public_subnet" {
  vpc_id            = aws_vpc.jobin_vpc.id
  cidr_block        = "10.0.1.0/24"
  availability_zone = "us-east-1f"

  tags = {
    Name = "jobin-public-subnet"
    Comment = "learning terraform automations for support needs"
  }
}

resource "aws_subnet" "jobin_private_subnet_1f" {
  vpc_id            = aws_vpc.jobin_vpc.id
  cidr_block        = "10.0.2.0/24"
  availability_zone = "us-east-1f"

  tags = {
    Name = "jobin-private-subnet_1f"
    Comment = "learning terraform automations for support needs"
  }
}

resource "aws_subnet" "jobin_private_subnet_1e" {
  vpc_id            = aws_vpc.jobin_vpc.id
  cidr_block        = "10.0.3.0/24"
  availability_zone = "us-east-1e"

  tags = {
    Name = "jobin-private-subnet_1e"
    Comment = "learning terraform automations for support needs"
  }
}

resource "aws_internet_gateway" "jobin_ig" {
  vpc_id = aws_vpc.jobin_vpc.id

  tags = {
    Name = "jobin-internet-gateway"
    Comment = "learning terraform automations for support needs"
  }
}

resource "aws_route_table" "jobin_public_rt" {
  vpc_id = aws_vpc.jobin_vpc.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.jobin_ig.id
  }

  route {
    ipv6_cidr_block = "::/0"
    gateway_id      = aws_internet_gateway.jobin_ig.id
  }

  tags = {
    Name = "jobin-public-route-table"
    Comment = "learning terraform automations for support needs"
  }
}

resource "aws_route_table_association" "jobin_public_1_rt_a" {
  subnet_id      = aws_subnet.jobin_public_subnet.id
  route_table_id = aws_route_table.jobin_public_rt.id
}

resource "aws_security_group" "jobin_web_sg" {
  name   = "HTTP and SSH"
  vpc_id = aws_vpc.jobin_vpc.id

  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = -1
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "jobin_pg_sg" {
  name   = "PG and SSH"
  vpc_id = aws_vpc.jobin_vpc.id

  ingress {
    from_port   = 5432
    to_port     = 5432
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = -1
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_instance" "jobin-priv-instance" {
  ami           = "ami-0f2eb5749e5a5699e"
  instance_type = "t2.nano"
  key_name      = "pgdemo"

  subnet_id                   = aws_subnet.jobin_private_subnet_1f.id
  vpc_security_group_ids      = [aws_security_group.jobin_web_sg.id]


  tags = {
    Name = "jobin-priv-instance"
    Comment = "learning terraform automations for support needs"
    "PerconaCreatedBy" = "yoann.lacancellera@percona.com"
  }
}

resource "aws_instance" "jobin-web-instance" {
  ami           = "ami-0f2eb5749e5a5699e"
  instance_type = "t2.nano"
  key_name      = "pgdemo"

  subnet_id                   = aws_subnet.jobin_public_subnet.id
  vpc_security_group_ids      = [aws_security_group.jobin_web_sg.id]
  associate_public_ip_address = true

  user_data = <<-EOF
  #!/bin/bash -ex

  dnf install -y nginx
  dnf install -y postgresql15
  echo '<h1>Hello, World!</h1>' >  /usr/share/nginx/html/index.html 
  systemctl enable nginx
  systemctl start nginx
  EOF

  tags = {
    Name = "jobin-web-instance"
    Comment = "learning terraform automations for support needs"
    "PerconaCreatedBy" = "yoann.lacancellera@percona.com"
  }
}

resource "aws_db_subnet_group" "jobin_pg_subnet_group" {
  name       = "jobin_pg_main_subnet"
  subnet_ids = [aws_subnet.jobin_private_subnet_1f.id, aws_subnet.jobin_private_subnet_1e.id]

  tags = {
    Name = "jobin_pg_subnet_group"
  }
}

resource "aws_db_instance" "jobin_pg" {
  allocated_storage    = 10
  db_name              = "jobin"
  engine               = "postgres"
  engine_version       = "15.3"
  instance_class       = "db.t3.micro"
  vpc_security_group_ids = [aws_security_group.jobin_pg_sg.id]
  db_subnet_group_name = aws_db_subnet_group.jobin_pg_subnet_group.name 
  username             = "foo"
  password             = "foobarbaz"
  skip_final_snapshot  = true
}

resource "aws_rds_cluster" "jobin_aurora_postgresql" {
  cluster_identifier      = "jobin-postgresql-cluster"
  engine                  = "aurora-postgresql"
  engine_version          = "14.4"
  vpc_security_group_ids  = [aws_security_group.jobin_pg_sg.id]
  db_subnet_group_name    = aws_db_subnet_group.jobin_pg_subnet_group.name
  database_name           = "jobin"
  master_username         = "foo"
  master_password         = "foobarbaz"
  skip_final_snapshot     = true
}

resource "aws_rds_cluster_instance" "aurora_postgresql_instance" {
  cluster_identifier           = aws_rds_cluster.jobin_aurora_postgresql.id
  instance_class               = "db.r5.large"  # Match the cluster instance class
  engine                       = aws_rds_cluster.jobin_aurora_postgresql.engine
  engine_version               = aws_rds_cluster.jobin_aurora_postgresql.engine_version
  db_subnet_group_name         = aws_db_subnet_group.jobin_pg_subnet_group.name
}
