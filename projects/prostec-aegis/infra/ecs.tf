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
  cpu                      = 256
  memory                   = 512
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  # PLACEHOLDER: replace with Aegis API image when built.
  # TODO on real image: readonlyRootFilesystem=true, user="1000:1000",
  #   linuxParameters.capabilities.drop=["ALL"], privileged=false,
  #   proper healthCheck (CMD /health), pin image to digest.
  container_definitions = jsonencode([{
    name      = "api"
    image     = "public.ecr.aws/nginx/nginx:latest"
    essential = true

    portMappings = [{
      containerPort = 80
      protocol      = "tcp"
    }]

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
  name                              = "${local.name_prefix}-api"
  cluster                           = aws_ecs_cluster.main.id
  task_definition                   = aws_ecs_task_definition.api.arn
  desired_count                     = 2
  launch_type                       = "FARGATE"
  platform_version                  = "LATEST"
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
    container_port   = 80
  }

  depends_on = [aws_lb_listener.https]

  # task_definition is managed by CI/CD; Terraform manages service config only.
  lifecycle {
    ignore_changes = [task_definition]
  }
}
