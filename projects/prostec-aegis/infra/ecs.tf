resource "aws_ecs_cluster" "main" {
  name = "${local.name_prefix}-cluster"

  setting {
    name  = "containerInsights"
    value = "enabled"
  }
}

resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name       = aws_ecs_cluster.main.name
  capacity_providers = ["FARGATE", "FARGATE_SPOT"]

  default_capacity_provider_strategy {
    capacity_provider = "FARGATE"
    weight            = 1
  }
}

resource "aws_ecs_task_definition" "api" {
  family                   = "${local.name_prefix}-api"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  # PLACEHOLDER: replace with Aegis API image when built.
  container_definitions = jsonencode([{
    name      = "api"
    image     = "public.ecr.aws/nginx/nginx:latest"
    essential = true

    # Fix H4: container port corrected from 80 to 8080
    portMappings = [{
      containerPort = 8080
      hostPort      = 8080
      protocol      = "tcp"
    }]

    # Fix H15: drop all Linux capabilities, run as non-root, read-only root filesystem
    readonlyRootFilesystem = true
    user                   = "1000:1000"

    linuxParameters = {
      capabilities = {
        drop = ["ALL"]
      }
    }

    # Fix M10: container health check — curl the /health endpoint
    healthCheck = {
      command     = ["CMD-SHELL", "wget -qO- http://localhost:8080/health || exit 1"]
      interval    = 30
      timeout     = 5
      retries     = 3
      startPeriod = 60
    }

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = "/ecs/${local.name_prefix}"
        "awslogs-region"        = var.aws_region
        "awslogs-stream-prefix" = "api"
      }
    }

    environment = [
      { name = "APP_ENV", value = var.environment },
      { name = "LOG_LEVEL", value = "info" },
    ]

    # ECS injects this from Secrets Manager at task startup via the execution role.
    # Value is a JSON blob; Aegis API parses it on boot.
    # If this secret is moved to a CMK, add kms:Decrypt on that key to ecs_execution role.
    secrets = [
      { name = "APP_CONFIG_JSON", valueFrom = aws_secretsmanager_secret.app_config.arn }
    ]
  }])
}

resource "aws_ecs_service" "api" {
  name    = "${local.name_prefix}-api"
  cluster = aws_ecs_cluster.main.id

  task_definition                   = aws_ecs_task_definition.api.arn
  desired_count                     = 2
  launch_type                       = "FARGATE"
  # Fix M9: pin to a specific Fargate platform version instead of LATEST
  platform_version                  = "1.4.0"
  health_check_grace_period_seconds = 30

  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  network_configuration {
    subnets          = aws_subnet.private[*].id
    security_groups  = [aws_security_group.ecs.id]
    assign_public_ip = false
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
    container_name   = "api"
    # Fix H4: align with corrected container port
    container_port   = 8080
  }

  depends_on = [aws_lb_listener.https]

  # task_definition is managed by CI/CD; Terraform manages service config only.
  lifecycle {
    ignore_changes = [task_definition]
  }
}
