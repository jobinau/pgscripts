terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

resource "aws_instance" "my_instance" {
  ami = "ami-07d9b9ddc6cd8dd30"
  instance_type = "t2.micro"
  key_name = "pgdemo"
  # ... other instance configurations

  # Get availability zone from instance
  tags = {
    Name = "ExampleDBServerInstance"
  }
}

locals {
  valid_device_names = [
    "/dev/sdh", "/dev/sdi", "/dev/sdj", "/dev/sdk", "/dev/sdl",
    "/dev/sdm", "/dev/sdn", "/dev/sdo", "/dev/sdp", "/dev/sdq",
  ]
}

resource "aws_ebs_volume" "my_volume" {
  count = 10  # Create 10 EBS volumes

  availability_zone = aws_instance.my_instance.availability_zone
  size               = 10  # Volume size in GiB
  type               = "gp2"  # Volume type (e.g., gp2 for General Purpose SSD)

  # Use element function to create unique volume names
  tags = {
    Name = format("my-volume-%d", count.index + 1)
  }
}

resource "aws_volume_attachment" "attach_volume" {
  count = 10  # Attach volume to instance 10 times

  #device_name = format("/dev/sd%c", count.index + ord("d"))  # Generate device names (sdx, sdy, ...)
  #device_name = format("/dev/sd%02d", count.index + 68)
  device_name   = local.valid_device_names[count.index]
  volume_id   = element(aws_ebs_volume.my_volume.*.id, count.index)
  instance_id = aws_instance.my_instance.id
}
